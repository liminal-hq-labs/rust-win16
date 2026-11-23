/* Linker script for Win16: one code segment and one data segment, each starting at offset 0.
 *
 * (c) Copyright 2026 Liminal HQ, Scott Morris
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 */
ENTRY(_start)

SECTIONS {
  OVERLAY 0 : AT(0x1000) {
    .text {
      *(.text.start)
      *(.text .text.*)
    }
    .data {
      KEEP(*(.data.00_reserved))
      *(.rodata .rodata.*)
      *(.data .data.*)
      *(.bss .bss.*)
      *(COMMON)
    }
  }
  /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) *(.gcc_except_table*) }
}
