# How the packer and the import stubs work

The build has three stages: `rustc` and `rust-lld` produce a 32-bit x86 ELF, `tools/ne_pack.py` rewraps it as a Win16 NE executable, and `tools/gen_imports.py` has already written the stubs that let Rust call Windows. This page explains each, and the Windows rules behind the odd parts.

## Table of contents

- [The target and the linker script](#the-target-and-the-linker-script)
- [The ELF the packer reads](#the-elf-the-packer-reads)
- [The NE file it writes](#the-ne-file-it-writes)
- [Symbols the packer reads](#symbols-the-packer-reads)
- [Import stubs](#import-stubs)
- [Entry point and exported procedures](#entry-point-and-exported-procedures)
- [Resources](#resources)
- [Size limits](#size-limits)
- [Reading and testing the output](#reading-and-testing-the-output)

## The target and the linker script

`win16.json` is a custom target whose `llvm-target` is `i386-unknown-none-code16`. LLVM emits 16-bit code and uses the operand-size prefix (`66`) and address-size prefix (`67`) whenever Rust wants a 32-bit register or address, so a near pointer is a 32-bit value whose high half stays zero and a call between Rust functions is a 32-bit near call. Floating point is off and the panic strategy is abort. `.cargo/config.toml` builds `core` and `alloc` from source for it with `-Zbuild-std`, which needs `RUSTC_BOOTSTRAP=1` on a stable compiler.

`link.ld` has to make a program whose code and data both start at offset 0 of their own segment, which `lld` normally refuses as overlapping. One `OVERLAY 0 : AT(0x1000)` block holding `.text` and `.data` links. `.bss` and `.rodata` are folded into `.data`, because code reaches its strings through DS. The first 16 bytes of the data segment are reserved, since Windows writes there.

## The ELF the packer reads

The packer reads section headers and the symbol table, not program headers. `.text` becomes code segment 1 and `.data` becomes the automatic data segment 2. It needs nothing else from the ELF: the linker has already resolved every address inside a segment.

## The NE file it writes

In file order:

- **MZ stub.** A 128-byte DOS header and a program that prints "This program requires Microsoft Windows." The NE header's offset sits at 0x3C.
- **NE header.** Flags `0x0302` (Windows API, multiple data), automatic data segment 2, `SS:SP = 2:0`, an 8 KB stack and a 4 KB heap by default, the entry point from `_start`, and counts and offsets for the tables below.
- **Segment table.** Code is a fixed, preloaded segment with relocations. Data is the automatic data segment.
- **Resource table**, **resident names**, **module-reference table**, **imported-names table**, **entry table** and **non-resident names**, in that order, each directly after the last.
- **Code segment**, followed by its relocation records, then the **data segment** (resource data follows it). Segment and resource offsets are in 16-byte units, so each starts on a 16-byte boundary.

## Symbols the packer reads

| Symbol | Meaning |
|---|---|
| `__w16imp_<MODULE>_<ordinal>_<Name>` | The four-byte operand of a far call to an import. The packer writes an imported-ordinal fixup of type far address for it |
| `__w16exp_<Name>` | The start of an exported far procedure. The packer writes an entry-table entry and a resident name |
| `__w16sel_code_<n>`, `__w16sel_data_<n>` | A 16-bit word in code that must hold the code or data segment's selector. The packer writes a selector fixup. Implemented and tested, unused by the examples |
| `_start` | The entry point, stored as `CS:IP` in the header |

Fixups that are not additive form chains: Windows reads the offset of the next fixup from the site it is about to patch, and 0xFFFF ends the chain. Every site here is a chain of one, so the packer writes 0xFFFF into each site. Leaving zeros makes the loader walk to offset 0 and fail with "Insufficient memory to run this application".

## Import stubs

`tools/imports.tbl` lists one import per line as `MODULE ORDINAL Name ARGS RET`. Ordinals and signatures come from Wine's `krnl386.exe16.spec`, `user.exe16.spec` and `gdi.exe16.spec`. `ARGS` has one letter per Rust argument, in Pascal (left to right) order:

| Letter | Argument |
|---|---|
| `w` | A 16-bit word, the low 16 bits of the argument |
| `l` | A 32-bit long, or a far pointer already built as `selector:offset` |
| `p` | A near pointer into the data segment, pushed as `DS:offset` |
| `f` | A near pointer into the code segment, pushed as `CS:offset` |

`RET` is `w` (AX, zero-extended), `l` (DX:AX folded into EAX) or `v` (nothing).

`tools/gen_imports.py` writes `src/imports.rs`: a `global_asm!` stub and an `extern "C"` declaration per row. Rust calls a stub with a C-style 32-bit near call and 4-byte arguments. The stub saves EBX, ESI and EDI, rebuilds the arguments as the Pascal convention wants them, makes the far call through the labelled operand, clears the high half of ESP, reloads ES from DS and returns the result in EAX.

To add an API, add a row to `tools/imports.tbl`, run `bash scripts/build.sh --regen`, and check the row against Wine:

```sh
curl -o /tmp/krnl.spec https://gitlab.winehq.org/wine/wine/-/raw/master/dlls/krnl386.exe16/krnl386.exe16.spec
python3 tools/gen_imports.py --check-spec DIR tools/imports.tbl /tmp/imports.rs
```

`--check-spec` takes the directory holding `krnl.spec`, `user.exe16.spec` and `gdi.exe16.spec`, and checks every ordinal and argument count.

## Entry point and exported procedures

`_start` in `src/lib.rs` must execute INITTASK as its first instruction. INITTASK reads the registers the loader set up (DS = SS, CX = the stack size, DI = 0), so nothing may run before it. It returns the instance handles, the command line and the show state. `_start` then calls `WaitEvent(0)` and `InitApp(hInstance)`, runs the program's `w16_main(hinst, hprev, show)` and exits through INT 21h function 4Ch.

A window or dialog procedure is an exported far procedure that Windows calls with the Pascal convention on the application's own stack. `export_proc!(Name, rust_fn)` builds that wrapper: it loads DS and ES from SS (so no `MakeProcInstance` is needed), saves EBX, ESI and EDI, clears the high half of ESP and passes the four words as 32-bit arguments to `rust_fn(hwnd, msg, wparam, lparam) -> u32`. Because it takes DS from SS, it is right for a program's own stack and has not been tested with two instances.

## Resources

`--resources FILE.json` takes dialogs, strings and raw data, each with an integer ID:

- **dialogs** have a caption, a position and size in dialog units, optional style names, and items with a class (`button`, `edit`, `static`, `listbox`, `scrollbar`, `combobox`), an ID, text, geometry and style names.
- **strings** map an ID to text. The packer groups them into the 16-string blocks the string table uses.
- **rcdata** maps an ID to a file, resolved relative to the JSON file.

A resource table entry stores its length in alignment units, so a resource whose length is not a multiple of 16 has to be padded, or Windows finds the file too short and a dialog silently fails to open. The padding means the true length is lost, so raw data starts with its exact length as a little-endian 32-bit number, and the program reads that first. `examples/setup` shows the read.

## Size limits

The packer refuses a code segment over 64 KB, or data plus stack plus heap over 64 KB. It does not split code across segments.

## Reading and testing the output

`tools/ne_dump.py` is a separate parser. It prints the header, segments, relocations, entry table, names and resources of any NE file. `tests/test_ne_pack.py` builds a synthetic ELF, packs it and checks the result with the dumper: header fields, relocation bytes, the entry table, names, resources, the size limits and the stub generator's output. It needs only Python.

```sh
python3 -m unittest discover -s tests -v
```
