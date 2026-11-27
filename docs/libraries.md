# Writing Windows 3.x libraries in Rust

A Win16 DLL is called far, on its caller's stack, and has one data segment that every task shares. The `w16dll` crate is the runtime that makes Rust safe to run in one, `tools/ne_pack.py --library` writes the file (see [packer.md](packer.md#building-a-library)), and `examples/hellodll` and `examples/dllhost` are a library and the program that loads it.

## Table of contents

- [Why a library needs its own runtime](#why-a-library-needs-its-own-runtime)
- [What the runtime does](#what-the-runtime-does)
- [Writing a library](#writing-a-library)
- [Calling it from a program](#calling-it-from-a-program)
- [Rules for a library](#rules-for-a-library)
- [What Windows does](#what-windows-does)
- [Testing](#testing)

## Why a library needs its own runtime

A program's stack is its data segment, so SS = DS, and LLVM's `code16` output relies on that: every pointer, including a pointer to a local variable, is addressed through DS. A library has no stack of its own. Windows calls an export on the caller's stack, where SS is the caller's segment and DS is the library's, and the two differ. Rust code that ran there would read its own locals from the wrong segment.

So Rust never runs on the caller's stack. Each export is a thunk written in assembly that moves to a private stack inside the library's data segment, on a 32-bit alias of that segment (the same device the program runtime uses, see [packer.md](packer.md#the-32-bit-stack)). With SS (the alias) and DS covering the same bytes, the compiled code is built exactly as it is for a program.

The library never calls Windows. Windows would run on a stack it did not make, KERNEL reads stack bounds from the task's stack, and a call that yields would let another task enter the library on the same private stack. The packer refuses an import that is not on the manifest's allow-list, and the example's list is empty. The caller locks memory and passes far pointers; the library reads and writes them.

## What the runtime does

**`LibEntry`** runs once, when the library is first loaded, with DS = the library's data segment and DI = the instance handle. It stores the handle, paints the private stack with `0xA5` so that `stack_margin()` can measure it, and returns 1. It makes no alias, so it cannot fail on DPMI.

**`WEP`** is ordinal 1 with a resident name. Windows calls it when the last user frees the library and at system exit. It loads DS, frees the alias and returns 1, and calls nothing else.

**`export_dll!(Name, rust_fn, words)`** defines the thunk, 53 bytes in code segment 1. It saves the caller's EBP, DS, ES, ESI, EDI and EBX (all 32 bits, since Rust keeps values in them and a 16-bit callee need not keep the high halves), loads DS from the library's data segment through a selector fixup, and calls `w16dll_enter`, which on the caller's stack:

1. refuses the call with a status when an earlier call panicked (`STATUS_INTERNAL`, 7) or another call is running (`STATUS_BUSY`, 2);
2. makes the 32-bit alias on the first call (DPMI 000Ah, 0009h with the big bit, 0008h for a 64 KB limit), and returns `STATUS_NOALIAS` (6) if DPMI refuses;
3. reads the data segment's base (DPMI 0006h) at every entry and moves the alias to it (0007h) when it differs, since Windows moves a moveable segment while it runs other code;
4. saves the caller's SS and SP, and copies the Pascal arguments to the top of the private stack, one zero-extended `u32` for each word, first word first.

The thunk then switches SS to the alias, ESP to the copied arguments, ES to DS, and calls the Rust function. On return, `w16dll_leave_ret` restores the caller's stack, moves the return address over the arguments, restores the registers and returns far with the result in DX:AX. One return sequence serves every parameter count because the argument size is kept in the data segment.

**A panic** calls `w16dll_poison`, which marks the library poisoned and leaves through the same return with `STATUS_INTERNAL`. Rust cannot unwind, so the caller sees a status instead of a crash, and every later call returns the same status until the library is freed and loaded again.

**Every `INT 31h` in the runtime is wrapped in a save and restore of DS.** Wine does not keep DS across a DPMI call (it assumes DS = SS), and Windows does, so the pair is two bytes of insurance on the one that matters.

## Writing a library

```rust
#![no_std]
#![no_main]
use w16dll::*;

export_dll!(Add, add, 2);

extern "C" fn add(a: u32, b: u32) -> u32 {
	a + b
}
```

The Rust function takes one `u32` for each word of the Pascal frame, in declaration order. A far pointer is two words, selector then offset. A long is two, high then low. The function returns a `u32` that becomes DX:AX. By convention an export returns a status in AX and writes anything else through a far pointer; the runtime reserves 2 (busy), 6 (no alias) and 7 (poisoned).

The manifest fixes the ordinals and repeats the word counts (`tests/test_dll.py` checks that the two agree):

```json
{
	"module": "HELLODLL",
	"gaps": "error",
	"imports": [],
	"exports": { "WEP": 1, "Add": { "ordinal": 2, "words": 2 } }
}
```

The crate is a package of its own in the workspace (`dll/`), because it brings the panic handler and the entry point `build.rs` selects (`--entry=LibEntry`), neither of which can share a binary with the program runtime's. A library is a `[[bin]]` of that package, built by `scripts/build.sh` and packed with `tools/ne_pack.py --library`. Each thunk leaves a zero-length reference to itself in a kept section (`.data.w16keep.*`), since nothing in the library refers to an export and the linker would otherwise discard it.

Helpers for far pointers are in the crate: `far_read8`, `far_write8`, `far_write16` and `far_write32`. `stack_margin()` returns how many bytes of the private stack have never been written, and `esp_dirty(n)` counts samples of ESP with a non-zero high half on the private stack (it should be 0).

## Calling it from a program

A program that must keep running without the library loads it itself, so a missing file is a status and never a box from the loader. LLVM cannot emit a far call, so the generated stubs include a dynamic kind: a row `DYN LIBRARY ORDINAL Name ARGS RET` in `tools/imports.tbl` makes a stub that calls through a 4-byte slot in the program's data segment, `__w16dyn_<Name>`, with the same argument rules as any import. An empty slot (a zero selector) drops the arguments and returns `0xFFFF`. The generator adds `<Name>_ORDINAL` and `bind_<Name>(far)` for the program to fill the slot with.

```rust
SetErrorMode(SEM_NOOPENFILEERRORBOX);
let h = LoadLibrary(path);
if h >= 32 {
	bind_HelloAdd(GetProcAddress(h, int_resource(HelloAdd_ORDINAL)));
}
let sum = HelloAdd(40_000, 40_000);
```

`LoadLibrary` (KERNEL 95), `FreeLibrary` (96), `GetProcAddress` (50), `GetModuleHandle` (47), `GetModuleUsage` (48), `WinExec` (166), `GetFreeSpace` (169), `AllocSelector` (175), `FreeSelector` (176) and USER's `GetTickCount` (13) are in the table, checked against Wine's spec files with `--check-spec`. Dynamic rows are not checked: Wine has no spec for a library of your own.

Always free the library: Windows 3.11 does not unload a library loaded with `LoadLibrary` when the task that loaded it ends (see below).

## Rules for a library

- **No Windows calls.** The manifest's `imports` is empty. See above for why.
- **Far pointers carry data.** A block comes from the caller's `GlobalAlloc`, is locked by the caller, and is passed as selector and offset. The library keeps no handle to it after the call returns.
- **One call at a time.** One private stack serves the library. A second entry while a call runs gets `STATUS_BUSY`. Windows 3.x is cooperative and the library never yields, so another task can enter only while the library is idle.
- **No callbacks.** The library never calls back into its caller.
- **One data segment for every task.** State the library keeps is shared by every program that has it loaded. Anything per task has to be passed in.
- **Module names are global.** A second `LoadLibrary` of a different file with the same module name returns the copy already loaded, whatever path was asked for, so a library needs its own version check (`Info` in the example returns the ABI major version, and the host refuses a mismatch).
- **Size.** Data (the 16 reserved bytes, statics, the 6,144-byte private stack and, when the library has more than one code segment, the 1 KB shadow stack) plus the heap is at most 64 KB, which leaves about 57 KB for the library's own statics and buffers. `HELLODLL.DLL` is 9,571 bytes, of which the runtime's code is about 410.
- **Code.** Segment 1 is fixed and holds the thunks and the runtime's helpers. Other code segments are moveable and preloaded; calls into them go through the packer's veneers and the shadow stack, which `Cross` in the example exercises directly and through a function pointer.

## What Windows does

Measured on Windows for Workgroups 3.11 in DOSBox-X (`dllhost`, `/nomove` aside):

- **Where it is found.** A bare name is searched in the task's current directory first (which is the program's own folder when `win D:\DLLHOST.EXE` starts it), then `WINDOWS\SYSTEM`, then the program's own folder. With the current directory elsewhere, a copy in `SYSTEM` wins over one beside the program. A full path from `GetModuleFileName` always finds the file beside the program.
- **Where it is loaded.** The data segment is moveable and its base is a high linear address at first; `GlobalCompact` moved it to low memory, and the alias followed it (DPMI 0007h) with every call correct afterwards.
- **Unloading.** `FreeLibrary` of the last user calls `WEP` and frees the module: a fresh load starts with its data reinitialized and the alias selector comes back the same, so `WEP` freed it. A task that ends without calling `FreeLibrary` leaves the library loaded.
- **Missing and mismatched files.** `LoadLibrary` returns 2 for a missing file and, with `SetErrorMode(SEM_NOOPENFILEERRORBOX)`, no box appears. A library of another ABI major version loads and the host's own check refuses it.
- **Several tasks.** Two tasks that load the library share one data segment, and each call runs on its own caller's stack.

## Testing

```sh
python3 -I -m unittest discover -s tests -v
CARGO_TARGET_DIR=/tmp/w16-target OUT_DIR=/tmp/w16-out bash scripts/build.sh
python3 tools/ne_check.py /tmp/w16-out/HELLODLL.DLL
W31_TREE=DIR DOSBOX_IMAGE=IMAGE bash scripts/run-w31.sh --runs 30 --log DLLHOST.LOG --with /tmp/w16-out/HELLODLL.DLL /tmp/w16-out/DLLHOST.EXE /exit
```

`DLLHOST.EXE` takes a mode on its command line: none (every check, library loaded by full path), `/bare` (by name, so the search order decides), `/cwd` (with `/bare`: make `C:\` the current directory first), `/missing`, `/wrongver` (with `HELLODLL2.DLL` renamed `HELLODLL.DLL`), `/share` (two tasks), `/leak` (load and end without freeing, watched by `/watch` in a second task), `/hold` (end Windows with the library loaded), `/count` (the number of free selectors in a fresh Windows, to compare with the count at the end of a full run) and `/nomove` (accept a library whose segment never moves, as under Wine). `/exit` ends Windows when the run is done. The log is `DLLHOST.LOG` beside the program, and ends `RESULT: PASS` or `RESULT: FAIL`.

The full run checks the version, an add that needs the high word of the result, a write of 1,000 bytes through a far pointer into a block the host allocated (and that byte 1,001 is untouched), state kept between calls, that the library's DS differs from the host's and equals the selector the loader patched in, that Rust ran on the alias and not on DS or the caller's stack, that the stack margin stays at 1,024 bytes or more, four million samples of ESP on the private stack, a call into a second code segment, a forced move of the data segment between calls, a panic and the poisoned state after it, a reload, 200 load and unload cycles with the same alias selector every time and the free memory unchanged, and the number of free selectors against a fresh Windows.

Wine (`wine DLLHOST.EXE /nomove /exit`) runs the same checks. Wine never moves a segment, so the move is not exercised there.
