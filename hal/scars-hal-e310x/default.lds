OUTPUT_ARCH( "riscv" )

ENTRY( _start )

MEMORY
{
  flash (rxa!w) : ORIGIN = 0x20400000, LENGTH = 128M
  ram   (wxa!r) : ORIGIN = 0x80000000, LENGTH = 16K
}

INCLUDE scars.x

#INCLUDE device.x
PROVIDE(WATCHDOG = DefaultHandler);
PROVIDE(RTC = DefaultHandler);
PROVIDE(UART0 = DefaultHandler);
PROVIDE(UART1 = DefaultHandler);
PROVIDE(QSPI0 = DefaultHandler);
PROVIDE(QSPI1 = DefaultHandler);
PROVIDE(QSPI2 = DefaultHandler);
PROVIDE(GPIO0 = DefaultHandler);
PROVIDE(GPIO1 = DefaultHandler);
PROVIDE(GPIO2 = DefaultHandler);
PROVIDE(GPIO3 = DefaultHandler);
PROVIDE(GPIO4 = DefaultHandler);
PROVIDE(GPIO5 = DefaultHandler);
PROVIDE(GPIO6 = DefaultHandler);
PROVIDE(GPIO7 = DefaultHandler);
PROVIDE(GPIO8 = DefaultHandler);
PROVIDE(GPIO9 = DefaultHandler);
PROVIDE(GPIO10 = DefaultHandler);
PROVIDE(GPIO11 = DefaultHandler);
PROVIDE(GPIO12 = DefaultHandler);
PROVIDE(GPIO13 = DefaultHandler);
PROVIDE(GPIO14 = DefaultHandler);
PROVIDE(GPIO15 = DefaultHandler);
PROVIDE(GPIO16 = DefaultHandler);
PROVIDE(GPIO17 = DefaultHandler);
PROVIDE(GPIO18 = DefaultHandler);
PROVIDE(GPIO19 = DefaultHandler);
PROVIDE(GPIO20 = DefaultHandler);
PROVIDE(GPIO21 = DefaultHandler);
PROVIDE(GPIO22 = DefaultHandler);
PROVIDE(GPIO23 = DefaultHandler);
PROVIDE(GPIO24 = DefaultHandler);
PROVIDE(GPIO25 = DefaultHandler);
PROVIDE(GPIO26 = DefaultHandler);
PROVIDE(GPIO27 = DefaultHandler);
PROVIDE(GPIO28 = DefaultHandler);
PROVIDE(GPIO29 = DefaultHandler);
PROVIDE(GPIO30 = DefaultHandler);
PROVIDE(GPIO31 = DefaultHandler);
PROVIDE(PWM0CMP0 = DefaultHandler);
PROVIDE(PWM0CMP1 = DefaultHandler);
PROVIDE(PWM0CMP2 = DefaultHandler);
PROVIDE(PWM0CMP3 = DefaultHandler);
PROVIDE(PWM1CMP0 = DefaultHandler);
PROVIDE(PWM1CMP1 = DefaultHandler);
PROVIDE(PWM1CMP2 = DefaultHandler);
PROVIDE(PWM1CMP3 = DefaultHandler);
PROVIDE(PWM2CMP0 = DefaultHandler);
PROVIDE(PWM2CMP1 = DefaultHandler);
PROVIDE(PWM2CMP2 = DefaultHandler);
PROVIDE(PWM2CMP3 = DefaultHandler);
PROVIDE(I2C0 = DefaultHandler);

PROVIDE(_user_exception_handler = DefaultHandler);

PHDRS
{
  text PT_LOAD;
  data PT_LOAD;
  bss PT_LOAD;
}

ISR_STACK_SIZE = 4096;

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
