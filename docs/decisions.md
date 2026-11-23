# Decisions

Each decision is numbered and stays here even when superseded, with a note on what replaced it. Open questions move here once they are answered.

| # | Decision | Choice | Why |
|---|---|---|---|
| D1 | Home | A Liminal HQ Labs experiment in `liminal-hq-labs/rust-win16`, public from the first commit | Try the idea in the open before it earns a place in Liminal HQ |
| D2 | Licence | `Apache-2.0 OR MIT` | House licence, compatible with both ecosystems |
| D3 | Dependency updates | No Dependabot configuration until the project opts in | Labs repositories stay quiet by default. See the [Labs charter](https://github.com/liminal-hq-labs/.github/blob/main/docs/labs-charter.md#dependabot-is-opt-in) |
| D4 | Language and target | Rust `no_std` built for LLVM `i386-unknown-none-code16`, linked by `rust-lld`, rewrapped as an NE executable by our own packer | It gives 16-bit segments with 32-bit registers on a pinned, stock toolchain, and the same Rust code can share tooling with other projects. The cost is an unsupported use of LLVM that is meant for bootloaders |
| D5 | CPU floor | A 386 or later, in 386 Enhanced mode, with no Win32s | Rust code is full of operand-size and address-size prefixes, which a 286 treats as invalid opcodes. Windows 3.1 Standard mode on a 286 is out |
| D6 | Compiler and build | A pinned stable compiler (1.98.0) with `RUSTC_BOOTSTRAP=1`, `-Zbuild-std = ["core", "alloc"]`, `compiler-builtins-mem` and a JSON target | `-Zbuild-std` and `json-target-spec` are unstable. The pin keeps the unstable surface from moving under the build |
| D7 | Own workspace | The crate declares an empty `[workspace]` | The build needs a custom target and `-Zbuild-std`, so nothing else should inherit them |
| D8 | Linker layout | One `OVERLAY 0 : AT(0x1000)` block holding `.text` and `.data`; `.bss` and `.rodata` folded into `.data`; the first 16 bytes of data reserved | Code and data both start at offset 0 of their own segment, which `lld` refuses without an overlay. Windows writes heap and stack bookkeeping into the first 16 bytes of the data segment, and without the reserve the first string in the program was overwritten |
| D9 | Own packer in Python | `tools/ne_pack.py` reads the ELF and writes the NE file, with `tools/ne_dump.py` as an independent reader in the tests | No linker writes NE from `rust-lld` output. The packer is short, has no dependencies and is tested against a separate parser |
| D10 | Import stubs | Generated from a table of Wine's ordinals and signatures, checked against Wine's spec files | A hand-written stub per API is error-prone. The table is the one place a signature lives |
| D11 | Register discipline | Every stub and `export_proc!` saves EBX, ESI and EDI, clears the high half of ESP and reloads ES from DS | On Windows 3.11 and ME, BX is replaced by some calls, the high half of ESP can come back non-zero after a yield, and ES is not kept. Wine keeps everything, so it hid all three. See the pitfalls in the README |
| D12 | Entry point | `_start` calls INITTASK as its very first instruction, then `WaitEvent(0)` and `InitApp` | INITTASK reads registers the loader set. Instructions before it changed what it saw and killed the task |
| D13 | Fixup chains | The packer writes 0xFFFF into every non-additive fixup site | Windows follows the chain through the site; zeros made the loader walk to offset 0 and report "Insufficient memory to run this application" |
| D14 | Resource data | Lengths are padded to the alignment and raw data carries a 4-byte length prefix | Resource lengths are in alignment units, and a short file made a dialog silently fail to open |
| D15 | Single segments | One code segment and one data segment, with the 64 KB limits enforced in the packer | Enough for the examples, and it keeps the packer small. Multi-segment code is not done |
| D16 | No Microsoft files | The repository ships no Windows files; testing on Windows 3.11 uses a tree the user supplies | The licensing is not ours to give |
| D17 | Verification | Examples are checked on Wine, on Windows for Workgroups 3.11 under DOSBox-X and on Windows ME in a virtual machine, never on real hardware | Wine tolerates mistakes real Windows rejects, so it is a smoke test. Real hardware, a 286 and Windows 3.0 remain open |
