// The Win16 library runtime: `LibEntry`, `WEP`, the export thunk with its private stack, the panic path and far-pointer helpers.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// A Win16 library is called far, on the caller's stack: SS is the caller's, DS is the library's own data segment, and the two are not
// the same segment. LLVM's `code16` output addresses every pointer, including pointers to locals, through DS, so Rust must never run
// on the caller's stack. Each export is therefore a thunk (`export_dll!`) that loads DS and ES from the library's own data segment,
// copies the Pascal arguments, moves to a private stack inside that data segment and runs the Rust function there on a 32-bit alias
// of the segment (SS and DS then cover the same bytes, which is what the compiler assumes). Rust never runs while SS is the caller's.
//
// The private stack, and the alias, are the same idea as the program runtime's (see `src/lib.rs`): ESP's high half is not kept on a
// 16-bit stack segment, so Rust runs on a 32-bit one. What differs is where the stack lives. A program's stack is its data segment,
// so SS = DS. A library has no stack of its own, so the thunk makes one.
//
// The library calls nothing in Windows: it computes, reading and writing the far blocks its caller passes. Its data segment can move
// only while Windows runs, so it cannot move during a call; the alias is checked against the segment's base at every entry.
#![no_std]

use core::arch::global_asm;
use core::ptr::addr_of;

/// The status every export returns, in AX, when it succeeds. A library's own statuses are its own; the runtime reserves two (below).
pub const STATUS_OK: u32 = 0;
/// An export was entered while another call was running. A single private stack serves one call at a time.
pub const STATUS_BUSY: u32 = 2;
/// DPMI refused the 32-bit alias, so the library cannot run Rust safely.
pub const STATUS_NOALIAS: u32 = 6;
/// The library panicked earlier: every call returns this until the library is unloaded and loaded again.
pub const STATUS_INTERNAL: u32 = 7;

/// The private stack's size in bytes. It lives in the data segment, so the segment is this much larger on disk.
pub const STACK_SIZE: usize = 6144;

/// The byte the stack is painted with when the library loads, to measure how deep it has gone.
const PAINT: u8 = 0xA5;

// Windows writes the local heap, atom table and stack bookkeeping into the first 16 bytes of the automatic data segment, so nothing of
// ours may live there. The linker script places this first.
#[used]
#[unsafe(link_section = ".data.00_reserved")]
static RESERVED: [u8; 16] = [0; 16];

// The runtime's state, in the library's data segment, and the private stack. `__w16d_alias` is the 32-bit alias of the data segment and
// `__w16d_base` the segment's base the alias was last made or moved to. `__w16d_caller_ss` and `__w16d_caller_sp` are the caller's stack
// while a call runs, `__w16d_argbytes` the size of the call's Pascal arguments and `__w16d_depth` 1 while a call runs.
global_asm!(
	".code16",
	".section .data.w16dll,\"aw\"",
	".balign 2",
	".globl __w16d_alias",
	"__w16d_alias: .word 0",
	".globl __w16d_caller_ss",
	"__w16d_caller_ss: .word 0",
	".globl __w16d_caller_sp",
	"__w16d_caller_sp: .word 0",
	".globl __w16d_argbytes",
	"__w16d_argbytes: .word 0",
	".globl __w16d_depth",
	"__w16d_depth: .word 0",
	".globl __w16d_poison",
	"__w16d_poison: .word 0",
	".globl __w16d_hinst",
	"__w16d_hinst: .word 0",
	".globl __w16d_moves",
	"__w16d_moves: .word 0",
	".globl __w16d_calls",
	"__w16d_calls: .word 0",
	".globl __w16d_base",
	"__w16d_base: .long 0",
	".section .bss.w16dll_stack,\"aw\",@nobits",
	".balign 16",
	".globl __w16dll_stack_base",
	"__w16dll_stack_base:",
	".space {size}",
	".globl __w16dll_stack_top",
	"__w16dll_stack_top:",
	size = const STACK_SIZE,
);

unsafe extern "C" {
	static __w16d_alias: u16;
	static __w16d_caller_ss: u16;
	static __w16d_caller_sp: u16;
	static __w16d_depth: u16;
	static __w16d_poison: u16;
	static __w16d_hinst: u16;
	static __w16d_moves: u16;
	static __w16d_calls: u16;
	static __w16d_base: u32;
	static __w16dll_stack_base: u8;
	fn w16dll_poison() -> !;
}

// `LibEntry`: Windows calls it once, far, when the library is first loaded, with DS = the library's data segment, DI = the instance
// handle, CX = the heap size and ES:SI = the command line. It stores the handle, paints the private stack, and returns AX = 1 (a zero
// fails the load on Windows; Wine ignores AX). It makes no alias, so it cannot fail on DPMI: the first call does that.
global_asm!(
	".code16",
	".section .text.start,\"ax\"",
	".globl LibEntry",
	"LibEntry:",
	"mov word ptr [__w16d_hinst], di",
	"push edi",
	"push es",
	"mov ax, ds",
	"mov es, ax",
	"cld",
	"mov edi, offset __w16dll_stack_base",
	"mov cx, {size}",
	"mov al, {paint}",
	"rep stosb",
	"pop es",
	"pop edi",
	"mov ax, 1",
	".byte 0xcb",
	size = const STACK_SIZE,
	paint = const PAINT,
);

// `WEP`: Windows calls it when the last user frees the library, or at system exit, with a word saying which. It frees the alias and
// returns 1. It calls nothing else: Windows 3.1 gives it a small stack and allows only a few KERNEL, USER and GDI calls. It is
// ordinal 1 with a resident name, since Windows looks it up by name.
global_asm!(
	".code16",
	".section .text.exp_WEP,\"ax\"",
	".globl WEP",
	".globl __w16exp_WEP",
	"WEP:",
	"__w16exp_WEP:",
	"push ds",
	"push ebx",
	".byte 0xb8",
	".globl __w16sel_data_WEP",
	"__w16sel_data_WEP:",
	".word 0xffff",
	"mov ds, ax",
	"xor bx, bx",
	"xchg bx, word ptr [__w16d_alias]",
	"or bx, bx",
	"jz 2f",
	"mov ax, 0x0001",
	"int 0x31",
	"2:",
	"pop ebx",
	"pop ds",
	"mov ax, 1",
	".byte 0xca, 0x02, 0x00",
	".section .data.w16keep.WEP,\"aw\"",
	".byte 0",
	".reloc 0, R_386_NONE, WEP",
);

// What every export thunk calls first, on the caller's stack with DS = the library's data segment and BP = the thunk's frame. It
// refuses a poisoned library or a nested entry, makes the 32-bit alias on the first call and moves it when the segment has moved, saves
// the caller's stack, and copies the Pascal arguments (one word each, zero-extended) to the top of the private stack in the order a C
// function wants them: the first Pascal word, which is the highest on the caller's stack, becomes the first C argument. It returns the
// stack pointer the thunk is to use in EAX. On a refusal it does not return: it leaves the thunk's frame with the status in AX.
//
// Every INT 31h is wrapped in a save and restore of DS. Wine does not keep DS across a DPMI call (it assumes DS = SS, which holds for a
// program and not for a library), and the data segment is what the next instruction reads. Windows keeps it, so the pair costs two bytes.
global_asm!(
	".code16",
	".section .text.w16fixed.dll_enter,\"ax\"",
	".globl w16dll_enter",
	"w16dll_enter:",
	"cmp word ptr [__w16d_poison], 0",
	"jne 8f",
	"cmp word ptr [__w16d_depth], 0",
	"jne 9f",
	"cmp word ptr [__w16d_alias], 0",
	"jne 5f",
	// DPMI 000Ah: an alias of the data segment; 0009h: the same access rights with the big (B) bit; 0008h: a 64 KB limit.
	"mov bx, ds",
	"mov ax, 0x000a",
	"push ds",
	"int 0x31",
	"pop ds",
	"jc 7f",
	"mov bx, ax",
	"lar cx, bx",
	"mov cl, ch",
	"mov ch, 0x40",
	"mov ax, 0x0009",
	"push ds",
	"int 0x31",
	"pop ds",
	"jc 6f",
	"xor cx, cx",
	"mov dx, 0xffff",
	"mov ax, 0x0008",
	"push ds",
	"int 0x31",
	"pop ds",
	"jc 6f",
	"mov word ptr [__w16d_alias], bx",
	// A new alias already has the segment's base: store it and count it as the first move.
	"mov bx, ds",
	"mov ax, 0x0006",
	"push ds",
	"int 0x31",
	"pop ds",
	"jc 7f",
	"mov word ptr [__w16d_base], dx",
	"mov word ptr [__w16d_base+2], cx",
	"inc word ptr [__w16d_moves]",
	"jmp 3f",
	// The segment's base, which the alias must follow: a base that differs from the one stored moves it.
	"5:",
	"mov bx, ds",
	"mov ax, 0x0006",
	"push ds",
	"int 0x31",
	"pop ds",
	"jc 7f",
	"cmp dx, word ptr [__w16d_base]",
	"jne 4f",
	"cmp cx, word ptr [__w16d_base+2]",
	"je 3f",
	"4:",
	"inc word ptr [__w16d_moves]",
	"mov word ptr [__w16d_base], dx",
	"mov word ptr [__w16d_base+2], cx",
	"mov bx, word ptr [__w16d_alias]",
	"mov ax, 0x0007",
	"push ds",
	"int 0x31",
	"pop ds",
	"3:",
	"inc word ptr [__w16d_calls]",
	"mov word ptr [__w16d_caller_ss], ss",
	"mov ax, sp",
	"add ax, 4",
	"mov word ptr [__w16d_caller_sp], ax",
	"mov word ptr [__w16d_depth], 1",
	"mov cx, word ptr [__w16d_argbytes]",
	"shr cx, 1",
	"movzx eax, cx",
	"shl eax, 2",
	"mov edi, offset __w16dll_stack_top",
	"sub edi, eax",
	"mov ebx, edi",
	"mov si, bp",
	"add si, 6",
	"add si, word ptr [__w16d_argbytes]",
	"mov ax, ss",
	"mov es, ax",
	"jcxz 2f",
	"1:",
	"movzx eax, word ptr es:[si]",
	"mov dword ptr [di], eax",
	"sub si, 2",
	"add di, 4",
	"loop 1b",
	"2:",
	"mov eax, ebx",
	".byte 0x66, 0xc3",
	"6:",
	"mov ax, 0x0001",
	"int 0x31",
	"7:",
	"mov ax, {noalias}",
	"jmp 20f",
	"8:",
	"mov ax, {internal}",
	"jmp 20f",
	"9:",
	"mov ax, {busy}",
	"20:",
	"add sp, 4",
	".byte 0x66, 0xe9",
	".long w16dll_leave_fail - . - 4",
	noalias = const STATUS_NOALIAS,
	internal = const STATUS_INTERNAL,
	busy = const STATUS_BUSY,
);

// What every export thunk ends in. `w16dll_leave_ret` is for a Rust function that returned (EAX = the result) and moves back to the
// caller's stack; `w16dll_leave_fail` is for a refusal (AX = the status), already on it. Both then restore the registers the thunk
// saved, drop the Pascal arguments and return far with the result in DX:AX. The return address is moved over the arguments first, so
// that one `retf` serves every parameter count.
global_asm!(
	".code16",
	".section .text.w16fixed.dll_leave,\"ax\"",
	".globl w16dll_leave_fail",
	"w16dll_leave_fail:",
	"movzx eax, ax",
	"jmp 1f",
	".globl w16dll_leave_ret",
	"w16dll_leave_ret:",
	"mov ss, word ptr [__w16d_caller_ss]",
	"mov sp, word ptr [__w16d_caller_sp]",
	"mov word ptr [__w16d_depth], 0",
	"1:",
	"mov edx, eax",
	"shr edx, 16",
	"mov cx, word ptr [__w16d_argbytes]",
	"mov bp, sp",
	"add bp, 16",
	"mov bx, bp",
	"add bx, cx",
	"mov si, word ptr ss:[bp+4]",
	"mov di, word ptr ss:[bp+6]",
	"mov word ptr ss:[bx+4], si",
	"mov word ptr ss:[bx+6], di",
	"pop ebx",
	"pop edi",
	"pop esi",
	"pop es",
	"pop ds",
	"pop ebp",
	"add sp, cx",
	".byte 0xcb",
);

// A panic, on the private stack: poison the library and leave with the internal status. Rust cannot unwind, so this is the whole
// handler. It ends the call the way a return does, so the caller sees a status, not a crash.
global_asm!(
	".code16",
	".section .text.w16fixed.dll_poison,\"ax\"",
	".globl w16dll_poison",
	"w16dll_poison:",
	"mov word ptr [__w16d_poison], 1",
	"mov eax, {internal}",
	".byte 0x66, 0xe9",
	".long w16dll_leave_ret - . - 4",
	internal = const STATUS_INTERNAL,
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
	unsafe { w16dll_poison() }
}

/// Defines an exported far procedure `$name` that calls the Rust function `$f`, which takes one `u32` for each word of the Pascal
/// frame (first argument first; a far pointer is two words, selector then offset, and a long is two, high then low) and returns a
/// `u32` that becomes DX:AX. `$words` is the number of words, which the manifest's `words` for the export must repeat.
///
/// The thunk saves the registers Windows expects back (and the whole of EBX, ESI, EDI and EBP, which Rust keeps values in), loads DS
/// from the library's own data segment, and lets `w16dll_enter` make the alias, copy the arguments and save the caller's stack. It
/// then switches to the private stack and calls `$f`. `w16dll_leave_ret` undoes it all and returns far, dropping the arguments.
#[macro_export]
macro_rules! export_dll {
	($name:ident, $f:path, $words:literal) => {
		::core::arch::global_asm!(
			".code16",
			concat!(".section .text.exp_", stringify!($name), ",\"ax\""),
			concat!(".globl ", stringify!($name)),
			concat!(".globl __w16exp_", stringify!($name)),
			concat!(stringify!($name), ":"),
			concat!("__w16exp_", stringify!($name), ":"),
			"push ebp",
			"mov bp, sp",
			"push ds",
			"push es",
			"push esi",
			"push edi",
			"push ebx",
			".byte 0xb8",
			concat!(".globl __w16sel_data_", stringify!($name)),
			concat!("__w16sel_data_", stringify!($name), ":"),
			".word 0xffff",
			"mov ds, ax",
			"mov word ptr [__w16d_argbytes], {bytes}",
			".byte 0x66, 0xe8",
			".long w16dll_enter - . - 4",
			"mov ss, word ptr [__w16d_alias]",
			"mov esp, eax",
			"mov ax, ds",
			"mov es, ax",
			"cld",
			".byte 0x66, 0xe8",
			".long {f} - . - 4",
			".byte 0x66, 0xe9",
			".long w16dll_leave_ret - . - 4",
			// Keeps the thunk through the linker's garbage collection, since nothing in the library refers to an export.
			concat!(".section .data.w16keep.", stringify!($name), ",\"aw\""),
			".byte 0",
			concat!(".reloc 0, R_386_NONE, ", stringify!($name)),
			bytes = const 2 * $words,
			f = sym $f,
		);
		unsafe extern "C" {
			pub fn $name();
		}
	};
}

/// The instance handle Windows gave the library.
pub fn instance() -> u32 {
	unsafe { addr_of!(__w16d_hinst).read() as u32 }
}

/// How many times the alias had to follow the data segment to a new place (the first call counts as one).
pub fn moves() -> u32 {
	unsafe { addr_of!(__w16d_moves).read() as u32 }
}

/// How many calls have entered an export since the library loaded (a 16-bit count that wraps).
pub fn calls() -> u32 {
	unsafe { addr_of!(__w16d_calls).read() as u32 }
}

/// The data segment's base as DPMI last reported it at an entry.
pub fn base() -> u32 {
	unsafe { addr_of!(__w16d_base).read() }
}

/// The selector of the caller's stack segment and the offset it was at when the call began (after the thunk's saved registers).
pub fn caller_stack() -> (u32, u32) {
	unsafe {
		(
			addr_of!(__w16d_caller_ss).read() as u32,
			addr_of!(__w16d_caller_sp).read() as u32,
		)
	}
}

/// 1 while a call runs, 0 outside one.
pub fn depth() -> u32 {
	unsafe { addr_of!(__w16d_depth).read() as u32 }
}

/// Whether an earlier panic has poisoned the library.
pub fn poisoned() -> bool {
	unsafe { addr_of!(__w16d_poison).read() != 0 }
}

/// The 32-bit alias selector the private stack runs on, and the data segment's own selector.
pub fn selectors() -> (u32, u32) {
	let ds: u16;
	unsafe {
		core::arch::asm!("mov ax, ds", out("ax") ds, options(nomem, nostack, preserves_flags))
	};
	(unsafe { addr_of!(__w16d_alias).read() as u32 }, ds as u32)
}

/// How many bytes at the bottom of the private stack have never been written: the margin the deepest call left.
pub fn stack_margin() -> u32 {
	let base = addr_of!(__w16dll_stack_base);
	let mut n = 0;
	while n < STACK_SIZE && unsafe { base.add(n).read() } == PAINT {
		n += 1;
	}
	n as u32
}

/// Reads one byte through a far pointer.
pub fn far_read8(sel: u32, off: u32) -> u8 {
	let v: u8;
	unsafe {
		core::arch::asm!(
			"mov es, dx",
			"mov al, es:[bx]",
			"push ds",
			"pop es",
			in("dx") sel as u16,
			in("bx") off as u16,
			out("al") v,
			options(nostack, preserves_flags),
		);
	}
	v
}

/// Writes one byte through a far pointer.
pub fn far_write8(sel: u32, off: u32, v: u8) {
	unsafe {
		core::arch::asm!(
			"mov es, dx",
			"mov es:[bx], al",
			"push ds",
			"pop es",
			in("dx") sel as u16,
			in("bx") off as u16,
			in("al") v,
			options(nostack, preserves_flags),
		);
	}
}

/// Writes one word through a far pointer.
pub fn far_write16(sel: u32, off: u32, v: u32) {
	unsafe {
		core::arch::asm!(
			"mov es, dx",
			"mov es:[bx], ax",
			"push ds",
			"pop es",
			in("dx") sel as u16,
			in("bx") off as u16,
			in("ax") v as u16,
			options(nostack, preserves_flags),
		);
	}
}

/// Writes a doubleword through a far pointer.
pub fn far_write32(sel: u32, off: u32, v: u32) {
	unsafe {
		core::arch::asm!(
			"mov es, dx",
			"mov es:[bx], eax",
			"push ds",
			"pop es",
			in("dx") sel as u16,
			in("bx") off as u16,
			in("eax") v,
			options(nostack, preserves_flags),
		);
	}
}

/// Spins for `rounds` iterations and counts how often the high half of ESP is not zero on the private stack. It should be zero: the
/// alias is a 32-bit stack segment, which keeps all of ESP across interrupts.
pub fn esp_dirty(rounds: u32) -> u32 {
	let dirty: u32;
	unsafe {
		core::arch::asm!(
			"push ebx",
			"xor ebx, ebx",
			"2:",
			"mov eax, esp",
			"shr eax, 16",
			"jz 3f",
			"inc ebx",
			"movzx esp, sp",
			"3:",
			"dec ecx",
			"jnz 2b",
			"mov eax, ebx",
			"pop ebx",
			inout("ecx") rounds => _,
			out("eax") dirty,
			options(nomem),
		);
	}
	dirty
}
