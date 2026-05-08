//! Per-context local namespace.
//!
//! Each execution context — thread, interrupt handler, event handler —
//! owns a type-keyed namespace. [`LocalStorage`] provides static-dispatch
//! access to the currently executing context's namespace; methods like
//! `LocalStorage::get::<T>()` look up `T` in the running context's
//! storage. [`LocalExecutor`] is the static-dispatch front-end for the
//! async executor installed in the current context.
//!
//! Same-priority handlers may share a single namespace via the
//! `with_shared_storage` builder methods, exposed through
//! [`SharedStorage`] / [`SharedStorageProvider`].
//!
//! # `LocalStorage` quick reference
//!
//! | Goal                                  | API                                                |
//! | ------------------------------------- | -------------------------------------------------- |
//! | Check presence                        | [`LocalStorage::contains`]`::<T>()`                |
//! | Read a `Copy` value                   | [`LocalStorage::get`]`::<T>()`                     |
//! | Write (drop old)                      | [`LocalStorage::set`]`::<T>(val)`                  |
//! | Write and return the old value        | [`LocalStorage::replace`]`::<T>(val)`              |
//! | Borrow briefly in a closure (`&T`)    | [`LocalStorage::with`]`::<T, _>(\|t\| ...)`        |
//! | Mutate briefly in a closure (`&mut T`)| [`LocalStorage::with_mut`]`::<T, _>(\|t\| ...)`    |
//! | RAII shared borrow (`&T`)             | [`LocalStorage::borrow`]`::<T>()`                  |
//! | RAII exclusive borrow (`&mut T`)      | [`LocalStorage::borrow_mut`]`::<T>()`              |
//! | Raw pointer (deref is `unsafe`)       | [`LocalStorage::as_ptr`]`::<T>()`                  |
//! | Install with a runtime value          | [`LocalStorage::put_init`]`(&CELL, val)`           |
//! | Install with a lazy init closure      | [`LocalStorage::put_init_with`]`(&CELL, \|\| ...)` |
//! | Install a const-initialized cell      | [`LocalStorage::put_take`]`(&CELL)`                |
//! | Link a previously-taken cell          | [`LocalStorage::put`]`(handle)`                    |
//! | Take a cell out (move to other slot)  | [`LocalStorage::remove`]`::<T>()`                  |
//! | Run a [`Publish`] source              | [`LocalStorage::publish`]`(&SRC)`                  |
//!
//! All access methods return `None` / `Err` if no entry of type `T` is
//! present. The `with` / `with_mut` closures additionally return `None`
//! when invoked re-entrantly against the same `T` (the node is unlinked
//! from the namespace while the closure runs — see *Mutability*).
//!
//! # Cell lifecycle
//!
//! Cells ([`LocalCell<T>`] / [`ConstLocalCell<T>`]) are first
//! *initialized* (or *taken*), yielding a move-only [`LocalHandle<T>`]
//! that represents exclusive ownership of an initialized-but-unlinked
//! cell. The handle can be passed to [`LocalStorage::put`] to link the
//! cell, and recovered later via [`LocalStorage::remove`]. The same
//! handle can then be put into a different namespace — cells move
//! between contexts over time.
//!
//! For the common one-shot pattern, [`LocalStorage::put_init`] /
//! [`LocalStorage::put_take`] hide the intermediate handle.
//!
//! # Mutability
//!
//! Two safe disciplines hand out `&T` / `&mut T`, both built on the
//! same trick: the node holding `T` is unlinked from the namespace
//! while the borrow is alive, so any nested lookup of the same type
//! returns `None`. Nothing else can reach the value until the borrow
//! ends and the node is reinserted — that's what makes the handed-out
//! reference sound.
//!
//! - **Closure-scoped** — [`LocalStorage::with`] / [`LocalStorage::with_mut`]
//!   take a closure and reinsert the node when the closure returns:
//!
//!   ```ignore
//!   LocalStorage::with_mut::<u32, _>(|c| *c += 1);
//!   ```
//!
//! - **RAII-scoped** — [`LocalStorage::borrow`] / [`LocalStorage::borrow_mut`]
//!   return a guard ([`LocalRef`] / [`LocalRefMut`]) that derefs to
//!   `&T` / `&mut T` and reinserts the node on drop. Use this when the
//!   borrow needs to span multiple statements that don't fit nicely
//!   inside one closure:
//!
//!   ```ignore
//!   let mut c = LocalStorage::borrow_mut::<u32>().unwrap();
//!   *c += 1;
//!   *c *= 2;
//!   // c dropped here, node reinserted
//!   ```
//!
//! Both disciplines forbid nested borrows of the same `T` — the second
//! call returns `None` because the first unlinked the node. Installing
//! a different value of the same type while a borrow is held is also
//! forbidden: it would leave the namespace with two entries for one
//! `TypeId` once the borrow drops, which the reinsert detects and
//! panics on.
//!
//! For `Copy` types, [`LocalStorage::get`] returns the value by copy
//! and [`LocalStorage::set`] / [`LocalStorage::replace`] write a new
//! value (`set` drops the old; `replace` returns it). These are safe by
//! construction — no reference is handed out.
//!
//! For cases that genuinely need a `&'static T` outliving the call,
//! [`LocalStorage::as_ptr`] returns `Option<*mut T>`. Obtaining the
//! pointer is safe; dereferencing it is `unsafe` and the caller must
//! guarantee no aliasing for the lifetime of any reference produced.
//!
//! # Publishing
//!
//! Higher-level setup uses [`Publish`]: a `'static` source describes
//! how to install one or more cells via [`Publish::try_publish_to`],
//! and callers run it through [`LocalStorage::publish`] /
//! [`LocalStorage::try_publish`]. See the [`publish`] submodule for
//! the writer-side contract and idiomatic patterns.

mod borrow;
mod cell;
mod executor;
mod list;
pub mod publish;
mod storage;

pub use borrow::{LocalRef, LocalRefMut};
pub use cell::{ConstLocalCell, LocalCell, LocalHandle};
pub use executor::LocalExecutor;
pub use publish::{Publish, PublishCtx, PublishError};
pub use storage::{LocalStorage, SharedStorage, SharedStorageProvider};
