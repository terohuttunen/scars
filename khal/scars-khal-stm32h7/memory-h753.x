/* STM32H753ZI: 2 MB flash, plus several RAM regions. Map the code at
 * the cortex-m boot vector window and the data into the first DTCM/AXI
 * block. The other RAM regions (SRAM2/3/4, backup) are left
 * unallocated for application use. */
MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 2048K
  RAM   (rwx) : ORIGIN = 0x24000000, LENGTH = 512K
}
