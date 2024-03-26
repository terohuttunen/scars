INCLUDE scars.x
INCLUDE memory.x

PHDRS
{
  text PT_LOAD;
  data PT_LOAD;
  bss PT_LOAD;
}

ISR_STACK_SIZE = 1024 * 4;

SECTIONS
{
  .text : {
    PROVIDE(_text_start = .);
    *(.text.init) *(.text .text.*)
    PROVIDE(_text_end = .);
    PROVIDE(main = abort);
  } >flash AT>flash :text

  # The GP register is set to point to _global_pointer. This is used for
  # GP-relative addressing of global symbols in .rodata, .data and
  # .bss-sections.
  PROVIDE(_global_pointer = .);

  .rodata : {
    PROVIDE(_rodata_start = .);
    *(.rodata .rodata.*)
    PROVIDE(_rodata_end = .);
  } >flash AT>flash :text

  .data : {
    . = ALIGN(4096);
    PROVIDE(_data_start = .);
    *(.sdata .sdata.*) *(.data .data.*)
    PROVIDE(_data_end = .);
  } >ram AT>ram :data

  .bss :{
    PROVIDE(_isr_stack_start = .);
    . = . + ISR_STACK_SIZE + 16;
    . = ALIGN(16);
    PROVIDE(_isr_stack_end = .);

    PROVIDE(_bss_start = .);
    *(.tasks .tasks.*)
    *(.sbss .sbss.*) *(.bss .bss.*)
    PROVIDE(_bss_end = .);
    . = ALIGN(16);
  } >ram AT>ram :bss

  PROVIDE(_memory_start = ORIGIN(ram));
  PROVIDE(_memory_end = ORIGIN(ram) + LENGTH(ram));
}