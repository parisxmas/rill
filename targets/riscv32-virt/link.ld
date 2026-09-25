/* QEMU `virt`, RISC-V 32: RAM at 0x80000000. The image first, then the heap,
   and the stack at the top. 512 KB in all — an ESP32's worth of RAM. */
ENTRY(_start)
MEMORY { RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 512K }
SECTIONS {
  .text : { *(.text.start) *(.text .text.*) } > RAM
  .rodata : { *(.rodata .rodata.* .srodata .srodata.*) } > RAM
  .data : { *(.data .data.*) __global_pointer$ = . + 0x800; *(.sdata .sdata.*) } > RAM
  .bss : { __bss_start = .; *(.sbss .sbss.* .bss .bss.* COMMON) . = ALIGN(4); __bss_end = .; } > RAM
  __heap_start = ALIGN(16);
  __stack_top = ORIGIN(RAM) + LENGTH(RAM);
  __heap_end = __stack_top - 32K;
}
