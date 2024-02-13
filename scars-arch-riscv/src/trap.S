.option norvc
.altmacro

.macro store_x i, basereg
	sw	x\i, ((\i) * 4)(\basereg)
.endm

.macro load_x i, basereg
	lw	x\i, ((\i) * 4)(\basereg)
.endm

.section .text
.global _start_first_task, _trap_vector
.align 4
_start_first_task:
	# First task context is the first and only parameter in a0
	mv		a5, a0

	# read mepc form context
	lw		a0, (64 * 4)(a5)

	# Set start scheduler syscall id to x17/a7
	sw      x0, (17 * 4)(a5)

	# mtval
	mv  	a1, x0
	# Machine Cause Register. Contains a code indicating the event that
	# caused the trap.
	li		a2, 11   # machine mode environment call
	# RISC-V hart id
	csrr	a3, mhartid
	# Machine Status Register. Interrupt-enable/disable etc.
	lw 	    a4, (65 * 4)(a5)

	la	    sp, _isr_stack_end
	
	# save task marker to ISR stack
	li      a6, 0
	addi    sp, sp, -4
	sw     	a6, 0(sp)
	jal    	kernel_trap_handler
	j		_enter_user
_trap_vector:
	# The current machine context is stored in the mscratch register.
	# When trap interrupts a task, the mscratch is pointing to the
	# context-variable in its TCB; in case of nested interrupts the
	# mscratch points to ISR stack where a MachineContext has been
	# allocated.

	# Swap mscratch and t6; this copies the machine context address
	# from mscratch to t6, while saving the original value of t6 to
	# mscratch.
	csrrw	t6, mscratch, t6

	# If t6 i.e. mscratch is NULL, there is no current task, or we
	# are in nested interrupt handler. If context address is
	# non-null, then jump directly to saving the current context
	# to the address.
	bnez	t6, save_context

	# If context address was null, then we are in nested trap, and
	# current context is saved into ISR stack.
nested_trap:
	# allocate context from ISR stack
	addi 	sp, sp, -(65*4)
	mv		t6, sp

save_context:
	# Save general purpose registers. No need to save x0, as it is
	# hardwired to zero.
	.set	i, 1
	.rept	30
		store_x	%i, t6
		.set	i, i+1
	.endr

	# Actual t6 is in mscratch, move machine context address from t6 to t5
	# which has already been saved, and read the actual t6 from mscratch
	# and save to context. Now t6 contains the machine context address; move
	# it to t5, and read original value of t6 from mscratch.
	mv 		t5, t6
	csrr	t6, mscratch
	store_x 31, t5

	# TBD: when to restore FPU registers

	# Per RISC-V calling convention, the callee is responsible for
	# saving and restoring other general purpose registers.
	# By default FPU is not allowed in trap handlers, and FPU registers
	# must be manually saved and restored by the callee as well.

	# store mepc CSR to context
	csrr	a0, mepc
	sw	a0, (64 * 4)(t5)

	# store mstatus CSR to context
	csrr a5, mstatus
	sw a5, (65 * 4)(t5)

	# Set context address in mscratch to null to denote that we are in a
	# trap. Nested trap will be detected from null mscratch.
	li		t6, 0
	csrw	mscratch, t6

	# Load kernel_trap_handler parameters to registers a0..a5.
	# Machine Exception Program Counter.

	# Machine trap value. Maybe zero, or contain some trap-specific
	# information such as faulting virtual address.
	csrr	a1, mtval
	# Machine Cause Register. Contains a code indicating the event that
	# caused the trap.
	csrr	a2, mcause
	# RISC-V hart id
	csrr	a3, mhartid
	# Machine Status Register. Interrupt-enable/disable etc.
	csrr	a4, mstatus

	# t5 contains the context address in task or interrupt stack
	# if t5 is equal to sp, then context was stored into interrupt stack
	bne	    t5, sp, task_context

	# set marker to 1 to denote that context is saved in ISR stack
	li      a6, 1
	j 		enter_handler

task_context:
	# Set stack pointer to the beginning of ISR stack
	# 1. interrupted task: mscratch is non-null
	#    - save context at mscratch, set sp to ISR stack, set mscratch to null, push marker 0 to ISR stack
	# 2. nested interrupt: mscratch is null
	#    - allocate context from sp, save context at sp, push marker 1 to ISR stack
	# 3. Call kernel_trap_handler
	# 4. pop marker from ISR stack to determine if context is pushed into ISR stack or task
	# 5. restore current context

	# context was stored into task
	la	    sp, _isr_stack_end
	li      a6, 0

enter_handler:
	mv		a5, t5
	# save marker to ISR stack
	addi    sp, sp, -4
	sw     	a6, 0(sp)

	jal    	kernel_trap_handler

_enter_user:

	# get marker from ISR stack
	lw      t5, 0(sp)
	addi    sp, sp, 4
	beqz    t5, restore_from_task

	# When marker is non-zero, context is restored from ISR stack
	# instead of the address returned by the kernel_trap_handler in a0
	mv 		a0, sp
restore_from_task:
	# The kernel trap handler returns a pointer to the machine context
	# to which we return from the trap in a0. Copy the address to mscratch.
	# First move value of a0 to t6 because t6 is the last register to
	# be restored later.
	mv 		t6, a0
	csrw	mscratch, t6

	# set mepc to context pc
	lw		t0, (64 * 4)(t6)
	csrw	mepc, t0

	# Restore mpie and mpp bits of mstatus register from context.
	lw 	    t0, (65 * 4)(t6)
	li      t1, 0x1880  # mask for mpie (7) and mpp (11,12) bits
	and     t0, t0, t1
	csrrc   x0, mstatus, t1   
	csrrs   x0, mstatus, t0

	# Restore general purpose registers. No need to restore x0, as it is
	# hardwired to zero. Note that t6 is used to hold the context address
	# because it is the last register to be restored from the context.
	.set	i, 1
	.rept	31
		load_x 	%i, t6
		.set	i, i+1
	.endr

	# TBD: when to restore FPU registers

	mret 
