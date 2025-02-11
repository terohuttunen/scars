use crate::kernel::hal::StackAlignment;
use aligned::Aligned;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use static_cell::ConstStaticCell;

const ELEMENT_SIZE: usize = core::mem::align_of::<StackAlignment>();

const CANARY_BYTE: u8 = 0x55;
const CANARY_SIZE: usize = if ELEMENT_SIZE > 16 { ELEMENT_SIZE } else { 16 };

#[allow(dead_code)]
#[derive(Clone, Copy)]
struct StackElement(Aligned<StackAlignment, [MaybeUninit<u8>; ELEMENT_SIZE]>);

impl StackElement {
    pub const fn size() -> usize {
        ELEMENT_SIZE
    }
}

pub struct Stack<const SIZE: usize>(
    ConstStaticCell<Aligned<StackAlignment, [MaybeUninit<u8>; SIZE]>>,
);

impl<const SIZE: usize> Stack<SIZE> {
    pub const fn new() -> Stack<SIZE> {
        Stack(ConstStaticCell::new(Aligned([MaybeUninit::uninit(); SIZE])))
    }

    pub fn init(&'static self) -> StackRefMut {
        let stack_array = self.0.take();
        let (canary_slice, stack_slice) = stack_array.split_at_mut(CANARY_SIZE);

        // Initialize stack memory
        let canary = MaybeUninit::fill(canary_slice, CANARY_BYTE);
        let initialized_stack = MaybeUninit::fill(stack_slice, 0);

        // Make sure that the stack top and bottom are properly aligned
        let (_prefix, stack, _suffix) = unsafe { initialized_stack.align_to_mut::<StackElement>() };

        StackRefMut { canary, stack }
    }
}

/// StackRef provides size-erased view to statically allocated Stack<SIZE>
pub struct StackRefMut {
    canary: &'static mut [u8],
    stack: &'static mut [StackElement],
}

impl StackRefMut {
    pub fn is_alive(&self) -> bool {
        self.canary.iter().all(|&b| b == CANARY_BYTE)
    }

    pub const fn bottom_ptr(&self) -> *const u8 {
        unsafe { self.stack.as_ptr().add(self.stack.len()) as *const _ }
    }

    pub const fn size(&self) -> usize {
        self.stack.len() * StackElement::size()
    }

    pub const fn alloc_size(&self) -> usize {
        self.size() + CANARY_SIZE
    }
}
