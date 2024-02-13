# Copyright (C) 2020 Ultic Oy

# Disable compressed instructions
.option norvc

.section .text.init
.global _start
_start:
	# Make sure supervisor address translation and protection is disabled
	# during the bootstrap for each hart until we are ready to turn MMU on.
	# TBD: this does not work without MMU
	# csrw	satp, zero

	# Park hart if not hart0.
	csrr	t0, mhartid
	bnez	t0, park

	# Set trap-handler vector
	la	t2, _trap_vector
	csrw	mtvec, t2

	# Initialize .bss-section to zero with 32bit stores
	# TODO: on 64bit architecture, use 64bit stores
	la	a0, _bss_start
	la	a1, _bss_end
	bgeu    a0, a1, skip_bss_init
bss_init:
	sw	zero, 0(a0)
	addi	a0, a0, 4
	bltu	a0, a1, bss_init
skip_bss_init:

	# Initialize stack pointer to end of isr stack area
	la	sp, _isr_stack_end

.option push
	# Set RISCV specific assembler options for `la` instruction
	# generation. Initial load of GP must be done unrelaxed, or
	# the linker will try to generate relaxed GP-relative loads.
	# Without relaxation the linker generates a non-relative load.
.option norelax
	la		gp, _global_pointer
.option pop
	# Make a non-returnable far-jump to kernel main written in Rust
	tail	init

	# Harts that do not bootstrap are parked here waiting for
	# an interrupt.
park:	wfi
	j	park


