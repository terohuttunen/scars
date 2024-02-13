use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

const CANARY_VALUE: (u64, u64) = (
    u64::from_be_bytes([0x55; core::mem::size_of::<u64>()]),
    u64::from_be_bytes([0x55; core::mem::size_of::<u64>()]),
);

#[repr(C)]
#[repr(align(16))]
pub struct StackCanary(u64, u64);

impl StackCanary {
    pub const fn new() -> StackCanary {
        StackCanary(CANARY_VALUE.0, CANARY_VALUE.1)
    }

    pub fn init(&mut self) {
        self.0 = CANARY_VALUE.0;
        self.1 = CANARY_VALUE.1;
    }

    #[inline]
    pub const fn is_alive(&self) -> bool {
        self.0 == CANARY_VALUE.0 && self.1 == CANARY_VALUE.1
    }
}

const CANARY_SIZE: usize = 16;

#[repr(align(16))]
#[repr(C)]
pub struct Stack<const MAX_DEPTH: usize>
where
    [u8; ((MAX_DEPTH + 16 / 2) / 16) * 16 + CANARY_SIZE]: Sized,
{
    stack: UnsafeCell<[MaybeUninit<u8>; ((MAX_DEPTH + 16 / 2) / 16) * 16 + CANARY_SIZE]>,
}

impl<const MAX_DEPTH: usize> Stack<MAX_DEPTH>
where
    [u8; ((MAX_DEPTH + 16 / 2) / 16) * 16 + CANARY_SIZE]: Sized,
{
    // Static check to assert that stack always has room for the canary
    #[allow(dead_code)]
    const SIZE_ALWAYS_GE_THAN_CANARY: bool = core::mem::size_of::<Self>() >= CANARY_SIZE;

    pub const fn new() -> Stack<MAX_DEPTH> {
        Stack {
            stack: UnsafeCell::new(unsafe { MaybeUninit::uninit().assume_init() }),
        }
    }

    /// Returns the size of the stack. Stack size is rounded upwards
    /// to nearest multiple of 16 bytes.
    pub const fn size(&self) -> usize {
        self.as_slice().len()
    }

    pub const fn bottom_ptr(&self) -> *const u8 {
        unsafe { self.as_slice().as_ptr().add(self.size()) }
    }

    pub const fn canary_ref(&self) -> &StackCanary {
        let (canary_slice, _stack_slice) = self.split();
        let canary_assume_init = unsafe { MaybeUninit::slice_assume_init_ref(canary_slice) };
        unsafe { &*(canary_assume_init as *const [u8] as *const StackCanary) }
    }

    pub const fn split(&self) -> (&[MaybeUninit<u8>; CANARY_SIZE], &[MaybeUninit<u8>]) {
        let full_slice = unsafe { &*self.stack.get() };
        // SAFETY: The formula that computes the stack size from MAX_DEPTH always adds
        // CANARY_SIZE to the allocated array size, and thus there should always be at
        // least CANARY_SIZE in the stack slice.
        let (canary_slice, stack_slice) = unsafe { full_slice.split_at_unchecked(CANARY_SIZE) };
        // SAFETY: a points to [MaybeUninit<u8>; CANARY_SIZE]? Yes it's [MaybeUninit<u8>] of
        // length CANARY_SIZE (CANARY_SIZE always added to the computed stack size, and checked statically).
        unsafe {
            (
                &*(canary_slice.as_ptr() as *const [MaybeUninit<u8>; CANARY_SIZE]),
                stack_slice,
            )
        }
    }

    pub const fn split_mut(
        &mut self,
    ) -> (&mut [MaybeUninit<u8>; CANARY_SIZE], &mut [MaybeUninit<u8>]) {
        let full_slice = unsafe { &mut *self.stack.get() };
        // SAFETY: The formula that computes the stack size from MAX_DEPTH always adds
        // CANARY_SIZE to the allocated array size, and thus there should always be at
        // least CANARY_SIZE in the stack slice.
        let (canary_slice, stack_slice) = unsafe { full_slice.split_at_mut_unchecked(CANARY_SIZE) };
        // SAFETY: a points to [MaybeUninit<u8>; CANARY_SIZE]? Yes it's [MaybeUninit<u8>] of
        // length CANARY_SIZE (CANARY_SIZE always added to the computed stack size, and checked statically).
        unsafe {
            (
                &mut *(canary_slice.as_mut_ptr() as *mut [MaybeUninit<u8>; CANARY_SIZE]),
                stack_slice,
            )
        }
    }

    pub const fn as_slice(&self) -> &[u8] {
        let (_canary_slice, stack_slice) = self.split();
        unsafe { MaybeUninit::slice_assume_init_ref(stack_slice) }
    }

    pub const fn as_mut_slice(&mut self) -> &mut [u8] {
        let (_canary_slice, stack_slice) = self.split_mut();
        unsafe { MaybeUninit::slice_assume_init_mut(stack_slice) }
    }

    pub const fn as_canary_slice(&self) -> &[u8; CANARY_SIZE] {
        let (canary_slice, _stack_slice) = self.split();
        unsafe {
            (*(canary_slice as *const [MaybeUninit<u8>; 16] as *const MaybeUninit<[u8; 16]>))
                .assume_init_ref()
        }
    }

    pub const fn as_mut_canary_slice(&mut self) -> &mut [u8; CANARY_SIZE] {
        let (canary_slice, _stack_slice) = self.split_mut();
        unsafe {
            (*(canary_slice as *mut [MaybeUninit<u8>; 16] as *mut MaybeUninit<[u8; 16]>))
                .assume_init_mut()
        }
    }

    pub const fn borrow(&'static self) -> StackRef {
        StackRef {
            canary: self.as_canary_slice(),
            stack: self.as_slice(),
        }
    }
}

// SAFETY: The type provides only const API, which is evaluated at compile
// time, and is therefore safe to call from any number of threads simultaneously
// (if references are counted properly)
unsafe impl<const MAX_DEPTH: usize> Sync for Stack<MAX_DEPTH> where
    [u8; ((MAX_DEPTH + 16 / 2) / 16) * 16 + CANARY_SIZE]: Sized
{
}

/// StackRef provides size-erased view to statically allocated Stack<SIZE>
pub struct StackRef {
    canary: &'static [u8],
    stack: &'static [u8],
}

impl StackRef {
    pub fn is_alive(&self) -> bool {
        true
    }

    pub const fn bottom_ptr(&self) -> *const u8 {
        unsafe { self.stack.as_ptr().add(self.stack.len()) }
    }

    pub const fn canary_ref(&self) -> &'static StackCanary {
        unsafe { &*(self.canary.as_ptr() as *const StackCanary) }
    }

    pub const unsafe fn canary_mut(&self) -> &'static mut StackCanary {
        &mut *(self.canary.as_ptr() as *mut u8 as *mut StackCanary)
    }

    pub const fn size(&self) -> usize {
        self.stack.len()
    }
}
