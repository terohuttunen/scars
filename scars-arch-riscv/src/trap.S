.option norvc
.altmacro

.section .text
.global _start_first_task, _trap_vector
.align 4
_start_first_task:
	# First task context is the first and only parameter in a0
	mv		a5, a0

	# Store first task context pointer to CURRENT_TASK_CONTEXT
	la 		a1, CURRENT_TASK_CONTEXT
	sw		a0, 0(a1)

	mv 		t6, a0

	csrw	mscratch, t6

	# set mepc to context pc
	lw		t0, (29 * 4)(t6)
	csrw	mepc, t0

	# Restore mpie and mpp bits of mstatus register from context.
	lw 	    t0, (30 * 4)(t6)
	li      t1, 0x1880  # mask for mpie (7) and mpp (11,12) bits
	and     t0, t0, t1
	csrrc   x0, mstatus, t1   
	csrrs   x0, mstatus, t0

	j		restore_callee
_trap_vector:
	# The current task context is stored in the mscratch register.
	# When trap interrupts a task, the mscratch is pointing to the
	# context-variable in its TCB. In nested interrupts mscratch
	# is NULL.

	# Swap mscratch and t6; this copies the machine context address
	# from mscratch to t6, while saving the original value of t6 to
	# mscratch.
	csrrw	t6, mscratch, t6

	# If t6 i.e. mscratch is NULL, there is no current task, or we
	# are in nested interrupt handler. If context address is
	# non-null, then jump directly to saving the current context
	# to the address.
	bnez	t6, save_context_to_task

	# If context address was null, then we are in nested trap, and
	# current context is saved into ISR stack.
save_context_to_stack:
	# Allocate context from ISR stack for caller registers.
	# Allocate 4 bytes more than necessary to align stack pointer
	# by 8 bytes after pushing the marker.
	addi 	sp, sp, -(17*4)

	sw		x1, (0*4)(sp)
	sw		x5, (1*4)(sp)
	sw		x6, (2*4)(sp)
	sw		x7, (3*4)(sp)
	sw		x10, (4*4)(sp)
	sw		x11, (5*4)(sp)
	sw		x12, (6*4)(sp)
	sw		x13, (7*4)(sp)
	sw		x14, (8*4)(sp)
	sw		x15, (9*4)(sp)
	sw		x16, (10*4)(sp)
	sw		x17, (11*4)(sp)
	sw		x28, (12*4)(sp)
	sw		x29, (13*4)(sp)
	sw		x30, (14*4)(sp)
	
	# original value of t6 is in mscratch
	csrr	t6, mscratch
	sw 		x31, (15*4)(sp)

	# Push marker to ISR stack to denote that context is saved to stack
	addi    sp, sp, -4
	li      a6, 1
	sw      a6, 0(sp)

	j 		enter_handler

save_context_to_task:
	# Save caller context to task. No need to save x0, as it is
	# hardwired to zero. Callee context is saved in context switch.
	# Also save x2 (aka. sp), because it is replaced by ISR stack pointer.
	sw		x1, (0*4)(t6)
	sw      x2, (1*4)(t6)
	sw		x5, (2*4)(t6)
	sw		x6, (3*4)(t6)
	sw		x7, (4*4)(t6)
	sw		x10, (7*4)(t6)
	sw		x11, (8*4)(t6)
	sw		x12, (9*4)(t6)
	sw		x13, (10*4)(t6)
	sw		x14, (11*4)(t6)
	sw		x15, (12*4)(t6)
	sw		x16, (13*4)(t6)
	sw		x17, (14*4)(t6)
	sw		x28, (25*4)(t6)
	sw		x29, (26*4)(t6)
	sw		x30, (27*4)(t6)

	mv		a0, t6
	jal		_save_interrupt_threshold

	# Actual t6 (aka. x31) is in mscratch, move machine context address from t6 to t5
	# which has already been saved, and read the actual t6 from mscratch
	# and save to context. Now t6 contains the machine context address; move
	# it to t5, and read original value of t6 from mscratch.
	mv 		t5, t6
	csrr	t6, mscratch
	sw      x31, (28*4)(t5)

	# store mepc CSR to task context
	csrr	a0, mepc
	sw		a0, (29 * 4)(t5)

	# store mstatus CSR to task context
	csrr 	a5, mstatus
	sw 		a5, (30 * 4)(t5)

	# Set stack pointer to ISR stack
	la	    sp, _isr_stack_end

	# Push current task context to ISR stack. This is later used
	# to determine if context switch is required.
	addi    sp, sp, -4
	sw		t5, 0(sp)

	# Push marker 0 to ISR stack to denote that when returning, context
	# should be restored from task.
	addi    sp, sp, -4
	sw      x0, 0(sp)

enter_handler:
	# Set context address in mscratch to null to denote that we are in a
	# trap handler. Nested trap will be detected from null mscratch.
	csrw	mscratch, x0

	# Load kernel_trap_handler parameters to registers a0..a2.
	# Machine Exception Program Counter.

	csrr	a0, mepc
	# Machine trap value. Maybe zero, or contain some trap-specific
	# information such as faulting virtual address.
	csrr	a1, mtval
	# Machine Cause Register. Contains a code indicating the event that
	# caused the trap.
	csrr	a2, mcause

	jal    	kernel_trap_handler

_enter_user:
	# Get marker from ISR stack:
	#  0 = restore from task context
	#  1 = restore from ISR stack
	lw      t5, 0(sp)
	addi    sp, sp, 4
	beqz    t5, return_to_task

return_to_interrupt:
	# When marker is non-zero, context is restored from ISR stack
	lw		x1, (0*4)(sp)
	lw		x5, (1*4)(sp)
	lw		x6, (2*4)(sp)
	lw		x7, (3*4)(sp)
	lw		x10, (4*4)(sp)
	lw		x11, (5*4)(sp)
	lw		x12, (6*4)(sp)
	lw		x13, (7*4)(sp)
	lw		x14, (8*4)(sp)
	lw		x15, (9*4)(sp)
	lw		x16, (10*4)(sp)
	lw		x17, (11*4)(sp)
	lw		x28, (12*4)(sp)
	lw		x29, (13*4)(sp)
	lw		x30, (14*4)(sp)
	lw		x31, (15*4)(sp)  # x31 == t6

	# sp (aka. x2) is not caller saved register
	addi	sp, sp, (16*4)

	ret
return_to_task:
	# Pop current task context before calling trap handler from the stack to t5
	lw 		t5, 0(sp)
	addi 	sp, sp, 4

	# Read current task context after trap handler to t6 (aka. x31)
	la      t0, CURRENT_TASK_CONTEXT
	lw      t6, 0(t0)

	# Returning to task from the outermost interrupt, set mscratch
	# to current context.
	csrw	mscratch, t6

	# set mepc to context pc
	lw		t0, (29 * 4)(t6)
	csrw	mepc, t0

	# Restore mpie and mpp bits of mstatus register from context.
	lw 	    t0, (30 * 4)(t6)
	li      t1, 0x1880  # mask for mpie (7) and mpp (11,12) bits
	and     t0, t0, t1
	csrrc   x0, mstatus, t1   
	csrrs   x0, mstatus, t0

	mv 		a0, t6
	jal		_restore_interrupt_threshold

	# If pre- and post- trap handler contexts are the same then there is no need
	# for context switch. Need only to restore caller saved registers.
	beq		t5, t6, restore_caller

	# Context switch required, store callee registers to task context before
	# restoring callee registers from the next task. Caller preserved registers
	# were already stored in the trap entry. Do not save sp, which was also
	# saved in trap entry.
	sw      x8, (5*4)(t5)
	sw      x9, (6*4)(t5)
	sw      x18, (15*4)(t5)
	sw      x19, (16*4)(t5)
	sw      x20, (17*4)(t5)
	sw      x21, (18*4)(t5)
	sw      x22, (19*4)(t5)
	sw      x23, (20*4)(t5)
	sw      x24, (21*4)(t5)
	sw      x25, (22*4)(t5)
	sw      x26, (23*4)(t5)
	sw      x27, (24*4)(t5)

restore_callee:
	# Callee-Saved Registers:
	# x8, x9, x18 to x27, but not x2 (see below).
	lw      x8, (5*4)(t6)
	lw      x9, (6*4)(t6)
	lw      x18, (15*4)(t6)
	lw      x19, (16*4)(t6)
	lw      x20, (17*4)(t6)
	lw      x21, (18*4)(t6)
	lw      x22, (19*4)(t6)
	lw      x23, (20*4)(t6)
	lw      x24, (21*4)(t6)
	lw      x25, (22*4)(t6)
	lw      x26, (23*4)(t6)	
	lw      x27, (24*4)(t6)

restore_caller:
	# Restore sp as it was clobbered by the ISR stack pointer
	lw		x2, (1*4)(t6)

	# Caller-Saved Registers:
	# All registers from x0 to x31 (except those listed as callee-saved)
	lw		x1, (0*4)(t6)
	lw		x5, (2*4)(t6)
	lw		x6, (3*4)(t6)
	lw		x7, (4*4)(t6)
	lw		x10, (7*4)(t6)
	lw		x11, (8*4)(t6)
	lw		x12, (9*4)(t6)
	lw		x13, (10*4)(t6)
	lw		x14, (11*4)(t6)
	lw		x15, (12*4)(t6)
	lw		x16, (13*4)(t6)
	lw		x17, (14*4)(t6)
	lw		x28, (25*4)(t6)
	lw		x29, (26*4)(t6)
	lw		x30, (27*4)(t6)
	lw		x31, (28*4)(t6)  # x31 == t6

	mret 
