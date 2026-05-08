//! Publishing services into a local-storage namespace.
//!
//! A publisher is a `'static` source that knows how to install one or
//! more handles into a [`StorageListHead`]. The user implements
//! [`Publish::try_publish_to`] using [`PublishCtx`] methods that
//! short-circuit with `?`. The trait's default [`Publish::publish_to`]
//! wraps the fallible form with a panic-on-error.
//!
//! # `PublishCtx` access discipline
//!
//! - **Read methods** (`contains` / `get` / `with` / `as_ptr` /
//!   `check_slot` / `require`) take `&self`. They are callable both
//!   from the body of [`Publish::try_publish_to`] and from inside the
//!   `make` closure of [`PublishCtx::put_init`].
//! - **Mutation methods** (`put_init` / `set` / `replace` /
//!   `with_mut`) take `&mut self`. They are only reachable from the
//!   body of `try_publish_to`. The `make` closure receives
//!   `&PublishCtx<'_>` (a shared reborrow), so the borrow checker
//!   keeps mutation out of value construction; cross-slot writes
//!   live where they are visible at the call site.
//!
//! # Idiomatic patterns
//!
//! Single slot:
//!
//! ```ignore
//! impl Publish for Service {
//!     fn try_publish_to(&'static self, ctx: &mut PublishCtx<'_>)
//!         -> Result<(), PublishError>
//!     {
//!         ctx.put_init(&self.cell, |_| ServiceHandle::new(self))?;
//!         Ok(())
//!     }
//! }
//! ```
//!
//! Multi-slot, check-then-commit (writer-enforced atomicity):
//!
//! ```ignore
//! fn try_publish_to(&'static self, ctx: &mut PublishCtx<'_>)
//!     -> Result<(), PublishError>
//! {
//!     ctx.check_slot::<BusHandle>()?;
//!     ctx.check_slot::<BusStats>()?;
//!     ctx.put_init(&self.handle_cell, |ctx| BusHandle::new(self, ctx))?;
//!     ctx.put_init(&self.stats_cell,  |_|   BusStats::default())?;
//!     Ok(())
//! }
//! ```
//!
//! With a published prerequisite:
//!
//! ```ignore
//! fn try_publish_to(&'static self, ctx: &mut PublishCtx<'_>)
//!     -> Result<(), PublishError>
//! {
//!     ctx.require::<CipherHandle>()?;
//!     ctx.put_init(&self.cell, |ctx| {
//!         FilterHandle::new(self, ctx.get::<CipherHandle>().unwrap())
//!     })?;
//!     Ok(())
//! }
//! ```

use super::cell::LocalCell;
use super::list::StorageListHead;

/// Error reported by [`PublishCtx`] operations.
#[derive(Debug, Clone, Copy)]
pub enum PublishError {
    /// A different cell of the same `T` is already in the namespace.
    AlreadyPublished(&'static str),
    /// A required type is not present in the namespace.
    MissingPrerequisite(&'static str),
}

impl core::fmt::Display for PublishError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AlreadyPublished(t) => {
                write!(f, "LocalStorage already contains type {}", t)
            }
            Self::MissingPrerequisite(t) => {
                write!(f, "missing prerequisite type {}", t)
            }
        }
    }
}

/// A `'static` source that publishes one or more handles into a local
/// storage namespace.
///
/// Implementations write [`Publish::try_publish_to`]; [`Publish::publish_to`]
/// is provided as a panic-on-error wrapper.
pub trait Publish: 'static {
    /// Error type. Defaults to [`PublishError`]; override to add
    /// domain-specific failure modes (e.g. hardware not responding).
    type Error: From<PublishError> = PublishError;

    /// Install this source's handles into the namespace addressed by
    /// `ctx`. Returns `Err` on collision, missing prerequisite, or any
    /// user-defined failure.
    ///
    /// **Atomicity is the writer's responsibility.** If full installation
    /// cannot be guaranteed, use [`PublishCtx::check_slot`] /
    /// [`PublishCtx::require`] up front to validate, then commit. A
    /// failure mid-commit leaves whatever was already installed.
    fn try_publish_to(&'static self, ctx: &mut PublishCtx<'_>) -> Result<(), Self::Error>;

    /// Panic-on-error wrapper around [`try_publish_to`].
    fn publish_to(&'static self, ctx: &mut PublishCtx<'_>)
    where
        Self::Error: core::fmt::Debug,
    {
        if let Err(e) = self.try_publish_to(ctx) {
            panic!(
                "{} failed to publish: {:?}",
                core::any::type_name::<Self>(),
                e
            );
        }
    }
}

/// Cursor-style argument passed to [`Publish::try_publish_to`].
///
/// Read methods take `&self` (callable inside the `make` closure of
/// [`put_init`](Self::put_init)); mutation methods take `&mut self`
/// (only callable from the body of `try_publish_to`). See the
/// [module docs](self) for the access discipline.
pub struct PublishCtx<'a> {
    head: &'a StorageListHead,
}

impl<'a> PublishCtx<'a> {
    /// Construct a context for `head`. Crate-internal: users obtain a
    /// `&mut PublishCtx<'_>` via the `try_publish` / `publish`
    /// entrypoints on [`StorageListHead`] / [`super::LocalStorage`].
    #[inline]
    pub(crate) fn new(head: &'a StorageListHead) -> Self {
        Self { head }
    }

    // -- Read methods (`&self`) --------------------------------------------

    /// Returns `true` if a value of type `T` is in the namespace.
    #[inline]
    pub fn contains<T: 'static>(&self) -> bool {
        self.head.contains::<T>()
    }

    /// Read a `Copy` value out of the namespace, or `None` if absent.
    #[inline]
    pub fn get<T: 'static + Copy>(&self) -> Option<T> {
        self.head.get::<T>()
    }

    /// Run `f` with shared access to the stored `T`. Returns `None` if
    /// `T` is absent or if a same-type lookup is already in flight (the
    /// node is unlinked while the closure runs — see
    /// [`StorageListHead::with`]).
    #[inline]
    pub fn with<T: 'static, R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.head.with::<T, R>(f)
    }

    /// Returns a raw pointer to the stored `T`, or `None` if absent.
    /// Dereferencing it is `unsafe`; prefer [`with`](Self::with) /
    /// [`get`](Self::get) when they fit.
    #[inline]
    pub fn as_ptr<T: 'static>(&self) -> Option<*mut T> {
        self.head.as_ptr::<T>()
    }

    /// Returns `Err(AlreadyPublished)` if `T` is already in the
    /// namespace, `Ok(())` otherwise. Use up front to validate
    /// multi-slot installs before committing any of them.
    #[inline]
    pub fn check_slot<T: 'static>(&self) -> Result<(), PublishError> {
        if self.head.contains::<T>() {
            Err(PublishError::AlreadyPublished(core::any::type_name::<T>()))
        } else {
            Ok(())
        }
    }

    /// Returns `Err(MissingPrerequisite)` if `T` is *not* in the
    /// namespace, `Ok(())` otherwise.
    #[inline]
    pub fn require<T: 'static>(&self) -> Result<(), PublishError> {
        if self.head.contains::<T>() {
            Ok(())
        } else {
            Err(PublishError::MissingPrerequisite(
                core::any::type_name::<T>(),
            ))
        }
    }

    // -- Mutation methods (`&mut self`) ------------------------------------

    /// Slot-check then commit. `make` is invoked exactly once on
    /// success; on collision it is not called and the cell is not
    /// initialized.
    ///
    /// `make` receives `&PublishCtx<'_>` (a shared reborrow of `self`),
    /// so it may read the namespace to compose its `T` but cannot
    /// invoke any of the mutation methods.
    #[inline]
    pub fn put_init<T: 'static>(
        &mut self,
        cell: &'static LocalCell<T>,
        make: impl FnOnce(&PublishCtx<'_>) -> T,
    ) -> Result<(), PublishError> {
        self.check_slot::<T>()?;
        // Reborrow as shared so `make` only sees the read surface.
        let val = make(&*self);
        self.head.put_init(cell, val);
        Ok(())
    }

    /// Write `val` into the namespace's stored `T`, dropping the
    /// previous value. Returns `Err(MissingPrerequisite)` if no entry
    /// of type `T` is present (the input is dropped on error).
    #[inline]
    pub fn set<T: 'static>(&mut self, val: T) -> Result<(), PublishError> {
        match self.head.set::<T>(val) {
            Ok(()) => Ok(()),
            Err(_dropped) => Err(PublishError::MissingPrerequisite(
                core::any::type_name::<T>(),
            )),
        }
    }

    /// Write `val` into the stored `T`, returning the previous value.
    /// `Err(MissingPrerequisite)` if no entry of type `T` is present
    /// (the input is dropped on error).
    #[inline]
    pub fn replace<T: 'static>(&mut self, val: T) -> Result<T, PublishError> {
        match self.head.replace::<T>(val) {
            Ok(prev) => Ok(prev),
            Err(_dropped) => Err(PublishError::MissingPrerequisite(
                core::any::type_name::<T>(),
            )),
        }
    }

    /// Run `f` with exclusive mutable access to the stored `T`. See
    /// [`StorageListHead::with_mut`] for the unlink-while-borrowed
    /// semantics. Returns `Err(MissingPrerequisite)` if `T` is absent
    /// (or, for re-entrant calls, currently unlinked).
    #[inline]
    pub fn with_mut<T: 'static, R>(
        &mut self,
        f: impl FnOnce(&mut T) -> R,
    ) -> Result<R, PublishError> {
        self.head
            .with_mut::<T, R>(f)
            .ok_or_else(|| PublishError::MissingPrerequisite(core::any::type_name::<T>()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::{ConstLocalCell, LocalCell};

    // Distinct types per test so the per-binary thread-local namespace
    // cannot interact with these private heads.
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct PA(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct PB(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct PC(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct PD(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct Missing;

    fn fresh() -> StorageListHead {
        StorageListHead::new()
    }

    #[test_case]
    fn ctx_reads() {
        let head = fresh();
        static CELL: ConstLocalCell<PA> = ConstLocalCell::new(PA(7));
        head.put(CELL.take());

        let ctx = PublishCtx::new(&head);
        assert!(ctx.contains::<PA>());
        assert_eq!(ctx.get::<PA>(), Some(PA(7)));
        assert_eq!(ctx.with::<PA, _>(|v| v.0), Some(7));
        assert!(ctx.as_ptr::<PA>().is_some());

        assert!(!ctx.contains::<Missing>());
        assert!(ctx.get::<Missing>().is_none());
        assert!(ctx.with::<Missing, _>(|_| ()).is_none());
        assert!(ctx.as_ptr::<Missing>().is_none());
    }

    #[test_case]
    fn check_slot_and_require() {
        let head = fresh();
        static CELL: ConstLocalCell<PA> = ConstLocalCell::new(PA(1));
        head.put(CELL.take());

        let ctx = PublishCtx::new(&head);
        // PA present: check_slot rejects, require accepts.
        assert!(matches!(
            ctx.check_slot::<PA>(),
            Err(PublishError::AlreadyPublished(_))
        ));
        assert!(ctx.require::<PA>().is_ok());

        // PB absent: check_slot accepts, require rejects.
        assert!(ctx.check_slot::<PB>().is_ok());
        assert!(matches!(
            ctx.require::<PB>(),
            Err(PublishError::MissingPrerequisite(_))
        ));
    }

    #[test_case]
    fn put_init_commits_on_success() {
        let head = fresh();
        static CELL: LocalCell<PA> = LocalCell::new();

        let mut ctx = PublishCtx::new(&head);
        ctx.put_init(&CELL, |_| PA(42)).unwrap();
        assert_eq!(head.get::<PA>(), Some(PA(42)));
    }

    #[test_case]
    fn put_init_does_not_run_make_on_collision() {
        let head = fresh();
        static EXISTING: ConstLocalCell<PA> = ConstLocalCell::new(PA(1));
        static OTHER: LocalCell<PA> = LocalCell::new();
        head.put(EXISTING.take());

        let mut ctx = PublishCtx::new(&head);
        let result = ctx.put_init(&OTHER, |_| -> PA {
            panic!("make ran despite slot collision");
        });
        assert!(matches!(result, Err(PublishError::AlreadyPublished(_))));
        assert_eq!(head.get::<PA>(), Some(PA(1)));
    }

    #[test_case]
    fn make_can_read_published_values() {
        let head = fresh();
        static A_CELL: ConstLocalCell<PA> = ConstLocalCell::new(PA(10));
        static B_CELL: LocalCell<PB> = LocalCell::new();
        head.put(A_CELL.take());

        let mut ctx = PublishCtx::new(&head);
        ctx.put_init(&B_CELL, |ctx| {
            // ctx is &PublishCtx — reads compile.
            let a = ctx.get::<PA>().expect("A must be visible to make");
            PB(a.0 + 5)
        })
        .unwrap();

        assert_eq!(head.get::<PB>(), Some(PB(15)));
    }

    #[test_case]
    fn writer_atomicity_via_check_slot_then_commit() {
        let head = fresh();
        static A_CELL: LocalCell<PA> = LocalCell::new();
        static B_CELL: LocalCell<PB> = LocalCell::new();

        // Pre-publish PB so the second slot collides.
        static B_PRE: ConstLocalCell<PB> = ConstLocalCell::new(PB(99));
        head.put(B_PRE.take());

        let mut ctx = PublishCtx::new(&head);

        let outcome: Result<(), PublishError> = (|| {
            ctx.check_slot::<PA>()?;
            ctx.check_slot::<PB>()?; // fails here
            ctx.put_init(&A_CELL, |_| PA(1))?;
            ctx.put_init(&B_CELL, |_| PB(2))?;
            Ok(())
        })();

        assert!(matches!(outcome, Err(PublishError::AlreadyPublished(_))));
        // Neither A_CELL nor B_CELL was touched.
        assert!(!head.contains::<PA>());
        assert_eq!(head.get::<PB>(), Some(PB(99)));
    }

    #[test_case]
    fn ctx_set_replace_with_mut() {
        let head = fresh();
        static CELL: ConstLocalCell<PA> = ConstLocalCell::new(PA(1));
        head.put(CELL.take());

        let mut ctx = PublishCtx::new(&head);

        ctx.set::<PA>(PA(2)).unwrap();
        assert_eq!(head.get::<PA>(), Some(PA(2)));

        let prev = ctx.replace::<PA>(PA(3)).unwrap();
        assert_eq!(prev, PA(2));
        assert_eq!(head.get::<PA>(), Some(PA(3)));

        ctx.with_mut::<PA, _>(|p| p.0 += 10).unwrap();
        assert_eq!(head.get::<PA>(), Some(PA(13)));
    }

    #[test_case]
    fn ctx_mutation_returns_missing_prerequisite_when_absent() {
        let head = fresh();
        let mut ctx = PublishCtx::new(&head);
        assert!(matches!(
            ctx.set::<PA>(PA(0)),
            Err(PublishError::MissingPrerequisite(_))
        ));
        assert!(matches!(
            ctx.replace::<PA>(PA(0)),
            Err(PublishError::MissingPrerequisite(_))
        ));
        assert!(matches!(
            ctx.with_mut::<PA, _>(|_| ()),
            Err(PublishError::MissingPrerequisite(_))
        ));
    }

    // A simple Publish impl exercising the full panic and try paths.
    struct ServiceP {
        cell: LocalCell<PC>,
    }
    impl ServiceP {
        const fn new() -> Self {
            Self {
                cell: LocalCell::new(),
            }
        }
    }
    impl Publish for ServiceP {
        fn try_publish_to(&'static self, ctx: &mut PublishCtx<'_>) -> Result<(), PublishError> {
            ctx.put_init(&self.cell, |_| PC(123))?;
            Ok(())
        }
    }

    #[test_case]
    fn head_try_publish_succeeds_then_collides() {
        static SERVICE: ServiceP = ServiceP::new();
        let head = fresh();

        head.try_publish(&SERVICE)
            .expect("first publish must succeed");
        assert_eq!(head.get::<PC>(), Some(PC(123)));

        // Second publish: cell is already initialized AND slot occupied.
        // The slot check fires first, returning AlreadyPublished.
        let result = head.try_publish(&SERVICE);
        assert!(matches!(result, Err(PublishError::AlreadyPublished(_))));
    }

    // Custom Error type via the associated_type_defaults override.
    #[derive(Debug)]
    enum DomainError {
        Publish(PublishError),
        DomainSpecific,
    }
    impl From<PublishError> for DomainError {
        fn from(e: PublishError) -> Self {
            Self::Publish(e)
        }
    }

    struct DomainSrc {
        cell: LocalCell<PD>,
        fail: bool,
    }
    impl Publish for DomainSrc {
        type Error = DomainError;
        fn try_publish_to(&'static self, ctx: &mut PublishCtx<'_>) -> Result<(), DomainError> {
            if self.fail {
                return Err(DomainError::DomainSpecific);
            }
            ctx.put_init(&self.cell, |_| PD(7))?;
            Ok(())
        }
    }

    #[test_case]
    fn custom_error_type_flows_through_try_publish() {
        static SRC_OK: DomainSrc = DomainSrc {
            cell: LocalCell::new(),
            fail: false,
        };
        static SRC_FAIL: DomainSrc = DomainSrc {
            cell: LocalCell::new(),
            fail: true,
        };

        let head = fresh();
        head.try_publish(&SRC_OK).unwrap();
        assert_eq!(head.get::<PD>(), Some(PD(7)));

        let other = fresh();
        let err = other.try_publish(&SRC_FAIL).unwrap_err();
        assert!(matches!(err, DomainError::DomainSpecific));
    }
}
