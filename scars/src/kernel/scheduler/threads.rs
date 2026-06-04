//! Thread scheduling: the ready/blocked/suspended run queues and the
//! block/yield/switch logic. Compiled only with `multithreading`.

use super::{
    ExecStateTag, RESCHEDULE_KIND_BLOCK_CURRENT, RESCHEDULE_KIND_YIELD_TO_EQUAL,
    RESCHEDULE_KIND_YIELD_TO_HIGHER, RawScheduler, Scheduler,
};
use crate::Instant;
use crate::kernel::exception::{KernelError, handle_kernel_error};
use crate::kernel::hal::set_current_thread_context;
use crate::kernel::list::LinkedList;
use crate::kernel::tracing;
use crate::priority::{Priority, PriorityOpt};
use crate::sync::PreemptLockKey;
use crate::sync::atomic::Ordering;
use crate::thread::{IDLE_THREAD_ID, RawThread, ThreadExecutionState};
use core::pin::Pin;

// Field projections and queue operations.
impl RawScheduler {
    pub(super) fn ready_queue(self: Pin<&Self>) -> Pin<&LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked(|s| &s.ready_queue) }
    }

    fn ready_queue_mut(self: Pin<&mut Self>) -> Pin<&mut LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.ready_queue) }
    }

    pub(super) fn blocked_list(self: Pin<&Self>) -> Pin<&LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked(|s| &s.blocked_list) }
    }

    fn blocked_list_mut(self: Pin<&mut Self>) -> Pin<&mut LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.blocked_list) }
    }

    pub(super) fn suspended_list(self: Pin<&Self>) -> Pin<&LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked(|s| &s.suspended_list) }
    }

    fn suspended_list_mut(self: Pin<&mut Self>) -> Pin<&mut LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.suspended_list) }
    }

    fn current_thread_mut(self: Pin<&mut Self>) -> &mut Pin<&'static RawThread> {
        unsafe { &mut self.get_unchecked_mut().current_thread }
    }

    // Queue operations.
    fn insert_to_ready_queue(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        thread.state.set(pkey, ThreadExecutionState::Ready);
        if thread.thread_id == self.idle_thread.thread_id {
            // Idle thread is always ready, but it is never inserted to ready queue.
            return;
        }
        tracing::thread_ready_begin(thread.as_thread_ref());
        let thread_priority = thread.priority(pkey);
        if thread.ceiling_lock_priority(pkey).is_none() {
            // If thread is not holding any locks, then thread goes to the back of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.priority.get(pkey) >= thread_priority
            });
        } else {
            // If thread is holding any locks, then it goes to the front of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.priority.get(pkey) > thread_priority
            });
        }
    }

    // Reinsert when priority changes
    #[cfg(feature = "priority-inheritance")]
    pub(super) fn reinsert_to_ready_queue(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        // Reinsert to ready queue
        self.as_mut().ready_queue_mut().remove(thread);
        let thread_priority = thread.priority(pkey);
        if thread.ceiling_lock_priority(pkey).is_none() {
            // If thread is not holding any locks, then thread goes to the back of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.priority.get(pkey) >= thread_priority
            });
        } else {
            // If thread is holding any locks, then it goes to the front of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.priority.get(pkey) > thread_priority
            });
        }
    }

    fn blocked_list_order(
        pkey: PreemptLockKey<'_>,
        thread: &RawThread,
        thread_priority: PriorityOpt,
    ) -> bool {
        let queue_thread_priority =
            unsafe { Pin::new_unchecked(thread).ceiling_lock_priority(pkey) };

        if queue_thread_priority.is_some() && thread_priority.is_some() {
            queue_thread_priority >= thread_priority
        } else if queue_thread_priority.is_some() {
            true
        } else if thread_priority.is_some() {
            false
        } else {
            false
        }
    }

    fn insert_to_blocked_queue(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        thread.state.set(pkey, ThreadExecutionState::Blocked);
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not block");
        }

        tracing::thread_ready_end(thread.as_thread_ref());
        let thread_priority = thread.ceiling_lock_priority(pkey);
        self.blocked_list_mut()
            .insert_after(thread, |queue_thread| {
                Self::blocked_list_order(pkey, queue_thread, thread_priority)
            });
    }

    #[cfg(feature = "priority-inheritance")]
    pub(super) fn reinsert_to_blocked_queue(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        // Reinsert to blocked list
        self.as_mut().blocked_list_mut().remove(thread);
        let thread_priority = thread.ceiling_lock_priority(pkey);
        self.blocked_list_mut()
            .insert_after(thread, |queue_thread| {
                Self::blocked_list_order(pkey, queue_thread, thread_priority)
            });

        // If thread is waiting in a queue, reinsert at its new priority.
        // If the queue's lock isn't acquirable from this context, defer
        // the reorder — the deferred-op path retries with the
        // then-current priority.
        if let Some(handle) = thread.wait_queue.get(pkey) {
            let entry = thread.get_wait_entry();
            if unsafe { handle.try_reinsert(pkey, entry) }.is_err() {
                thread.schedule_deferred_op(RawThread::OP_REINSERT_WAIT_QUEUE);
            }
        }
    }

    fn insert_to_suspended_list(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        thread.state.set(pkey, ThreadExecutionState::Suspended);
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not suspend");
        }

        tracing::thread_ready_end(thread.as_thread_ref());
        self.suspended_list_mut().push_back(thread);
    }
}

// Thread scheduling.
impl RawScheduler {
    /// Try to wakeup a thread. If removing the thread
    /// from the wait queue is not safe right now, returns Err(()).
    pub(crate) fn try_wakeup_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) -> Result<(), ()> {
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not be woken up");
        }

        // Per-thread flag for `Notify`-style timed waits. Set
        // unconditionally on entry so that, even if `try_remove`
        // below defers (ceiling violation) and the notify path races
        // ahead, the flag still reflects "this thread's wake was
        // timer-driven." Read and cleared by
        // `Scheduler::take_last_wait_timed_out`.
        thread.set_wait_timed_out(pkey, true);

        // Remove thread from a wait queue if it is waiting in one. If
        // the queue's lock isn't acquirable from this context, return
        // Err(()) so the caller can defer.
        if let Some(handle) = thread.wait_queue.get(pkey) {
            let wait_entry = thread.get_wait_entry();
            unsafe { handle.try_remove(pkey, wait_entry)? };
            thread.disarm_wait(pkey);
        }

        // Set timeout flag if thread has current wait events (indicating it timed out)
        let wait_events_ptr = thread
            .current_wait_events
            .load(crate::sync::atomic::Ordering::SeqCst);
        if !wait_events_ptr.is_null() {
            let wait_events = unsafe { &*wait_events_ptr };
            wait_events.set_timed_out();
        }

        match thread.state.get(pkey) {
            ThreadExecutionState::Blocked => {
                self.as_mut().blocked_list_mut().remove(thread);
                self.insert_to_ready_queue(pkey, thread);
            }
            _ => (),
        }

        // Note: does not check for need to reschedule, as this is called from reschedule.
        Ok(())
    }

    /// Try to resume a thread. If removing the thread
    /// from the wait queue is not safe right now, returns Err(()).
    pub(crate) fn try_resume_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) -> Result<(), ()> {
        // Remove thread from a wait queue if it is waiting in one. If
        // the queue's lock isn't acquirable from this context, return
        // Err(()) so the caller can defer.
        if let Some(handle) = thread.wait_queue.get(pkey) {
            let wait_entry = thread.get_wait_entry();
            unsafe { handle.try_remove(pkey, wait_entry)? };
            thread.disarm_wait(pkey);
        }

        match thread.state.get(pkey) {
            ThreadExecutionState::Ready | ThreadExecutionState::Running => {
                // Thread is already in ready queue or running
            }
            ThreadExecutionState::Blocked => {
                // Clear any wakeup deadline; this also removes the timer from
                // the timer queue and reprograms the alarm.
                thread.set_wakeup_deadline(pkey, self.as_mut(), None);
                self.as_mut().blocked_list_mut().remove(thread);
                self.as_mut().insert_to_ready_queue(pkey, thread);
            }
            ThreadExecutionState::Suspended => {
                self.as_mut().suspended_list_mut().remove(thread);
                self.as_mut().insert_to_ready_queue(pkey, thread);
            }
            ThreadExecutionState::Created => {
                // Created thread does not yet have a closure, so it cannot be resumed
                // until it becomes Started.
            }
            ThreadExecutionState::Started => {
                self.as_mut().insert_to_ready_queue(pkey, thread);
            }
        }

        let current_priority = self.current_thread.priority(pkey);
        let locks_priority = self.as_ref().locks_priority_ceiling(pkey);
        let min_priority = current_priority.max_valid(locks_priority);
        if min_priority < thread.priority(pkey) {
            Scheduler::set_pending_reschedule(RESCHEDULE_KIND_YIELD_TO_HIGHER)
        }
        Ok(())
    }

    fn block_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
        deadline: Option<Instant>,
    ) {
        thread.set_wakeup_deadline(pkey, self.as_mut(), deadline);
        self.insert_to_blocked_queue(pkey, thread);
    }

    pub(crate) fn suspend_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        maybe_thread: Option<Pin<&'static RawThread>>,
    ) {
        let thread = maybe_thread.unwrap_or(self.current_thread);

        match thread.state.get(pkey) {
            ThreadExecutionState::Ready => {
                self.as_mut().ready_queue_mut().remove(thread);
                self.as_mut().insert_to_suspended_list(pkey, thread);
            }
            ThreadExecutionState::Running => {
                // Highest priority of any locks held by the current or blocked threads.
                // Any ready thread above lock ceiling can run next. No lock should have
                // the minimum priority, so default lock priority to MIN.
                let locks_ceiling = self
                    .as_ref()
                    .locks_priority_ceiling(pkey)
                    .unwrap_or_default(Priority::MIN);

                // Highest priority ready thread that is above the lock ceiling
                // will be the next to run. Or if there is no ready thread above
                // the lock ceiling, then the idle thread will be the next to run.
                let next = self
                    .as_mut()
                    .ready_queue_mut()
                    .pop_front_if(|ready| ready.priority.get(pkey) > locks_ceiling)
                    .unwrap_or(self.as_ref().idle_thread);

                let previous = self.as_mut().switch_thread(pkey, next);
                self.as_mut().insert_to_suspended_list(pkey, previous);
            }
            ThreadExecutionState::Blocked => {
                // A blocked thread holds its locks and prevents tasks below its priority
                // from running until it releases the locks, even when suspended.
                self.as_mut().insert_to_suspended_list(pkey, thread);
            }
            ThreadExecutionState::Suspended => {
                // Thread is already suspended
            }
            ThreadExecutionState::Created => {
                // Created thread does not yet have a closure, so it cannot be suspended
                // until it becomes Started.
                panic!("Cannot suspend a thread that has not been started");
            }
            ThreadExecutionState::Started => {
                self.as_mut().insert_to_suspended_list(pkey, thread);
            }
        }

        // Re-merge the alarm with the current thread's monitor budget: the
        // `Running` arm switches to a new current thread without otherwise
        // reprogramming it.
        #[cfg(feature = "execution-time-monitor")]
        self.as_ref().reprogram_alarm(pkey);
    }

    fn check_stack_overflow(&self) {
        if !unsafe { self.current_thread.stack.assume_init_ref() }.is_alive() {
            let error = KernelError::StackOverflow {
                thread_name: self.current_thread.name,
                stack_size: unsafe { self.current_thread.stack.assume_init_ref().size() },
            };
            handle_kernel_error(&error);
        }
    }

    fn switch_thread(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        new: Pin<&'static RawThread>,
    ) -> Pin<&'static RawThread> {
        // Whenever current thread is switched out, check its stack canary for
        // stack overflow that could have occurred during the thread execution.
        self.check_stack_overflow();
        //printkln!("[scheduler] switching to thread {}", new.name);
        new.state.set(pkey, ThreadExecutionState::Running);

        let new_thread_ref = new.as_thread_ref();
        if new.thread_id == IDLE_THREAD_ID {
            tracing::system_idle();
        }

        set_current_thread_context(new.context.as_ptr());
        let old = core::mem::replace(self.current_thread_mut(), new);

        tracing::thread_exec_end(old.as_thread_ref());
        tracing::thread_exec_begin(new_thread_ref);
        old.state.set(pkey, ThreadExecutionState::Ready);

        old
    }

    fn locks_priority_ceiling(self: Pin<&Self>, pkey: PreemptLockKey<'_>) -> PriorityOpt {
        if let Some(blocked_thread) = self.blocked_list().head() {
            let blocked_prio = blocked_thread.ceiling_lock_priority(pkey);
            let current_prio = self.current_thread.ceiling_lock_priority(pkey);
            blocked_prio.max(current_prio)
        } else {
            self.current_thread.ceiling_lock_priority(pkey)
        }
    }

    /// Applies the thread-scheduling outcome of a reschedule once the
    /// timer queue has been drained: blocks the current thread, or
    /// switches to the highest-priority eligible ready thread, per `kind`.
    pub(super) fn reschedule_threads<'key>(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'key>,
        kind: usize,
    ) {
        // Drain-driven block: the commit site has armed the wait and
        // written `pending_block_deadline`. Any yield bits ORed in
        // are moot once we switch away from current.
        if (kind & RESCHEDULE_KIND_BLOCK_CURRENT) != 0 {
            self.as_mut().block_current(pkey);
            return;
        }

        let current_priority = self.current_thread.priority(pkey);
        // Threads at or below any held mutex's ceiling cannot run while
        // the holder is still in its critical section — otherwise they
        // could try to take the same mutex and the immediate-ceiling
        // protocol would be violated (panic at ceiling_lock.rs:140).
        // The blocking primitives below (`delay_thread_until`,
        // `wait_current_thread`, `wait_current_thread_event`) apply the
        // same filter; yield must do so too or a `thread_yield()` from
        // a boosted holder can hand the CPU to a same-priority peer
        // that immediately panics inside `lock()`.
        let locks_ceiling = self
            .as_ref()
            .locks_priority_ceiling(pkey)
            .unwrap_or_default(Priority::MIN);

        let next = if (kind & RESCHEDULE_KIND_YIELD_TO_EQUAL) != 0 {
            // Any thread at or above current priority, but strictly
            // above any held lock's ceiling.
            self.as_mut()
                .ready_queue_mut()
                .pop_front_if(|ready| {
                    let p = ready.priority.get(pkey);
                    p >= current_priority && p > locks_ceiling
                })
                .unwrap_or(self.current_thread)
        } else if (kind & RESCHEDULE_KIND_YIELD_TO_HIGHER) != 0 {
            // Any thread strictly above current priority, and above
            // any held lock's ceiling.
            self.as_mut()
                .ready_queue_mut()
                .pop_front_if(|ready| {
                    let p = ready.priority.get(pkey);
                    p > current_priority && p > locks_ceiling
                })
                .unwrap_or(self.current_thread)
        } else {
            unreachable!();
        };

        if next.thread_id != self.current_thread.thread_id {
            let previous = self.as_mut().switch_thread(pkey, next);
            self.insert_to_ready_queue(pkey, previous);
        }
    }

    pub(super) fn delay_thread_until(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        wakeup_time: u64,
    ) {
        if self.current_thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread cannot block");
        }

        // Highest priority of any locks held by the current or blocked threads.
        // Any ready thread above lock ceiling can run next.
        let locks_ceiling = self
            .as_ref()
            .locks_priority_ceiling(pkey)
            .unwrap_or_default(Priority::MIN);

        // Highest priority ready thread that is above the lock ceiling
        // will be the next to run. Or if there is no ready thread above
        // the lock ceiling, then the idle thread will be the next to run.
        let next = self
            .as_mut()
            .ready_queue_mut()
            .pop_front_if(|ready| ready.priority.get(pkey) > locks_ceiling)
            .unwrap_or(self.idle_thread);

        let previous = self.as_mut().switch_thread(pkey, next);
        self.as_mut()
            .block_thread(pkey, previous, Some(Instant { tick: wakeup_time }));
    }

    /// Drain-driven block: invoked from `reschedule` when the
    /// `RESCHEDULE_KIND_BLOCK_CURRENT` bit is set. The commit site
    /// (`Protected::with_barrier{,_until}`) is responsible for
    /// enqueuing the thread on a wait list (arming `wait_queue`) and
    /// for writing `pending_block_deadline` before pending the kind.
    fn block_current(mut self: Pin<&mut Self>, pkey: PreemptLockKey<'_>) {
        if self.current_thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread cannot block");
        }

        // Gate: notify or timer may have removed us between commit and
        // drain. On that path leave the deadline cell as-is (the next
        // commit overwrites or `take` resets it) and let
        // `wait_timed_out` stay false so the post-resume read returns
        // `Notified`.
        if self.current_thread.wait_queue.get(pkey).is_none() {
            return;
        }

        // From here we definitely suspend. Reset the per-thread
        // timed-out flag so the post-wake read reflects only this
        // suspend.
        self.current_thread.set_wait_timed_out(pkey, false);

        let deadline = self.current_thread.take_pending_block_deadline(pkey);

        let locks_ceiling = self
            .as_ref()
            .locks_priority_ceiling(pkey)
            .unwrap_or_default(Priority::MIN);

        let next = self
            .as_mut()
            .ready_queue_mut()
            .pop_front_if(|ready| ready.priority.get(pkey) > locks_ceiling)
            .unwrap_or(self.idle_thread);

        let blocked_thread = self.as_mut().switch_thread(pkey, next);

        self.block_thread(pkey, blocked_thread, deadline);
    }

    pub(crate) fn wait_current_thread_event(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        wait_events: *mut crate::WaitEvents,
        deadline: Option<u64>,
    ) {
        if self.current_thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread cannot block");
        }

        let wait_events = unsafe { &*wait_events };
        let waiting_thread = self.current_thread;

        // Set thread's current WaitEvents
        waiting_thread
            .current_wait_events
            .store(wait_events as *const _ as *mut _, Ordering::SeqCst);

        // Step 1: Read all currently pending events
        let all_pending = waiting_thread.pending_events.load(Ordering::SeqCst);

        // Step 2: Let WaitEvents process and store events atomically
        let (received_events, events_to_clear) =
            wait_events.process_and_store_pending_events(all_pending);

        // Step 3: Atomically clear the determined events
        waiting_thread
            .pending_events
            .fetch_and(!events_to_clear, Ordering::SeqCst);

        // Note: If there is a send_events call from ISR between the above operations
        // and blocking of the thread, then the ISR will put the thread into pending resume
        // queue, and the thread will be unblocked when the preemption lock is released.

        // Determine if thread should block based on WaitEvents configuration
        let should_block = wait_events.should_block(received_events);

        if should_block {
            // Highest priority of any locks held by the current or blocked threads.
            let locks_ceiling = self
                .as_ref()
                .locks_priority_ceiling(pkey)
                .unwrap_or_default(Priority::MIN);

            let next = self
                .as_mut()
                .ready_queue_mut()
                .pop_front_if(|ready| ready.priority.get(pkey) > locks_ceiling)
                .unwrap_or(self.idle_thread);

            let blocked_thread = self.as_mut().switch_thread(pkey, next);
            self.as_mut()
                .block_thread(pkey, blocked_thread, deadline.map(|tick| Instant { tick }));
        }
    }
}
