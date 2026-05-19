/* `_stext` must be set BEFORE `INCLUDE link.x` so that cortex-m-rt's
 * `PROVIDE(_stext = ADDR(.vector_table) + SIZEOF(.vector_table))` sees
 * the symbol as defined and no-ops. memory.x is included from inside
 * link.x, so we can't reference ORIGIN(FLASH) here — hardcode the Pico 2
 * flash base + space for vector table (≤0x110) and .start_block (0x14),
 * rounded to 0x140 for slack. */
_stext = 0x10000140;

INCLUDE scars.x
INCLUDE link.x
INCLUDE defmt.x

/* ── ISR stack layout ────────────────────────────────────────────────
 *
 * Top of RAM holds two MSP regions, one per core, both with sizes
 * configurable through linker symbols (defaults 1 KiB each, override
 * via `--defsym` rustflags or a higher-priority linker script).
 *
 * Core 0 sits at the very top of RAM so cortex-m-rt's
 * `_stack_start = _ram_end` default points straight at it — no need
 * to override `_stack_start`. We only override `_stack_end` to pin
 * the cortex-m-rt boot/ISR stack to exactly `_core0_isr_stack_size`
 * bytes. Plain assignment suppresses cortex-m-rt's PROVIDE (PROVIDE
 * is a no-op when the symbol is otherwise defined).
 *
 * Core 1's region sits directly below core 0's, ending at
 * `_stack_end` (= `_core1_stack_top`) and starting at
 * `_core1_stack_bottom`. `multicore::launch_core1` hands
 * `_core1_stack_top` to the boot-ROM hand-off as core 1's initial SP.
 *
 * Layout (high address at top):
 *     _stack_start (= _ram_end)        ← cortex-m-rt default
 *     ↓ core 0 boot/ISR stack ↓        (_core0_isr_stack_size bytes)
 *     _stack_end = _core1_stack_top
 *     ↓ core 1 ISR stack ↓             (_core1_isr_stack_size bytes)
 *     _core1_stack_bottom
 *     …
 *     __euninit                         ← top of .uninit
 *
 * The assert below catches the case where the configured sizes don't
 * leave room for .bss/.data/.uninit. */
PROVIDE(_core0_isr_stack_size = 1024);
PROVIDE(_core1_isr_stack_size = 1024);
_stack_end          = (ORIGIN(RAM) + LENGTH(RAM)) - _core0_isr_stack_size;
_core1_stack_top    = _stack_end;
_core1_stack_bottom = _core1_stack_top - _core1_isr_stack_size;

ASSERT(_core1_stack_bottom >= __euninit, "
ERROR(scars-khal-rp2350): _core0_isr_stack_size + _core1_isr_stack_size
exceed available RAM after .bss/.data/.uninit. Shrink one of the ISR
stack sizes (e.g. `--defsym=_core1_isr_stack_size=512`) or move data
out of RAM.");

/* RP2350 boot ROM scans the first 4 KiB of flash for an Image Definition
 * block (BLOCK_MARKER_START word). Place `.start_block` immediately after
 * the cortex-m-rt vector table — well within that window. */
SECTIONS {
    .start_block : ALIGN(4)
    {
        KEEP(*(.start_block));
    } > FLASH
} INSERT AFTER .vector_table;
