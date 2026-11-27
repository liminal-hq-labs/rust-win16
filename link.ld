/* Linker script for Win16: the data segment at offset 0, and every code section an output section of its own for tools/ne_pack.py.
 *
 * (c) Copyright 2026 Liminal HQ, Scott Morris
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * Linked with `--unique` (each code section not named here becomes an output section of its own), `--emit-relocs` (the relocations
 * stay in the output) and `--noinhibit-exec` (a 16-bit relative jump between sections can be out of range at the addresses used here;
 * the packer recomputes it, and fails on undefined symbols itself). The packer places the code sections in code segments and
 * recomputes every reference to code. Data is final as linked: code reaches it through DS at these offsets.
 */
ENTRY(_start)

SECTIONS {
  .data 0 : {
    KEEP(*(.data.00_reserved))
    KEEP(*(.data.w16keep.*))
    *(.rodata .rodata.*)
    *(.data .data.*)
    *(.bss .bss.*)
    *(COMMON)
  }
  /* Code is linked well above the data, so that a code address can never be mistaken for a data offset. */
  .text.start 0x100000 : { *(.text.start) }
  /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) *(.gcc_except_table*) }
}
