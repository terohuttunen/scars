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

/* RP2350 boot ROM scans the first 4 KiB of flash for an Image Definition
 * block (BLOCK_MARKER_START word). Place `.start_block` immediately after
 * the cortex-m-rt vector table — well within that window. */
SECTIONS {
    .start_block : ALIGN(4)
    {
        KEEP(*(.start_block));
    } > FLASH
} INSERT AFTER .vector_table;
