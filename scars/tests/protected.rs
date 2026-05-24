#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use scars::Stack;
use scars::prelude::*;
use scars::sync::{CoreCeilingLock, Protected, TryLockError};
use scars::thread::{Thread, ThreadFn};
use scars_test;

#[cfg(not(feature = "khal-sim"))]
const STACK_SIZE: usize = 1024;
#[cfg(feature = "khal-sim")]
const STACK_SIZE: usize = 16384;

const CHECKER_PRIORITY: Priority = Priority::thread(1);
const CEILING: Priority = Priority::thread(5);

type CheckerF = impl ThreadFn;

static CHECKER_STACK: Stack<STACK_SIZE> = Stack::new();
static CHECKER: Thread<CHECKER_PRIORITY, CheckerF> = Thread::new("checker");

static P: Protected<u32, CoreCeilingLock<CEILING>> = Protected::new(0);
static A: Protected<u32, CoreCeilingLock<CEILING>> = Protected::new(1);
static B: Protected<u32, CoreCeilingLock<CEILING>> = Protected::new(2);

/// Exercises `Protected::{with, with_key, try_with}` against a real
/// `CoreCeilingLock<CEILING>` to verify the wrapper integrates with an
/// actual nesting-lock primitive (priority raise via `L::with`, key
/// produced by the lock, re-entrancy gate via the inner `LockedCell`).
#[scars::init]
#[define_opaque(CheckerF)]
fn init() {
    CHECKER
        .init(CHECKER_STACK.init())
        .attach(move || {
            // with: read + mutate roundtrip.
            assert_eq!(P.with(|_, t| *t), 0);
            P.with(|_, t| *t = 7);
            assert_eq!(P.with(|_, t| *t), 7);

            // with_key: use a key already held from outer L::with;
            // skips redundant lock acquire.
            CoreCeilingLock::<CEILING>::with(|key| {
                assert_eq!(P.with_key(key, |_, t| *t), 7);
                P.with_key(key, |_, t| *t = 11);
            });
            assert_eq!(P.with(|_, t| *t), 11);

            // try_with: succeeds when uncontended.
            assert!(matches!(P.try_with(|_, t| *t * 2), Ok(22)));

            // try_with: same-task re-entry reports WouldBlock instead
            // of tripping RecursiveLock.
            let r = P.with(|_, _| P.try_with(|_, _| ()));
            assert!(matches!(r, Err(TryLockError::WouldBlock)));

            // Different instances compose naturally.
            assert_eq!(A.with(|_, ta| B.with(|_, tb| *ta + *tb)), 3);

            scars_test::test_succeed()
        })
        .start();
}
