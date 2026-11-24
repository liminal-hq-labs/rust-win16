// The Win16 runtime: the task entry point, the exported-procedure wrapper, the types the programs share and a few helpers.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// The code is built for an LLVM `code16` target: 16-bit segments, 32-bit registers through operand-size prefixes. Near pointers are
// 32-bit values whose high half is always zero; the data segment is DS = SS. Every call between Rust functions is a 32-bit near call
// (four bytes of return address), every Windows API is a far call made through a stub in `imports.rs`.
#![no_std]

use core::arch::{asm, global_asm};
use core::ptr::addr_of_mut;

#[rustfmt::skip]
pub mod imports;
pub mod probe;
pub mod wizard;
pub use imports::*;

pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_INITDIALOG: u32 = 0x0110;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_USER: u32 = 0x0400;
pub const IDOK: u32 = 1;
pub const IDCANCEL: u32 = 2;
pub const MB_OK: u32 = 0;
pub const MB_ICONINFORMATION: u32 = 0x40;
pub const MB_ICONSTOP: u32 = 0x10;
pub const PM_REMOVE: u32 = 1;
pub const SW_SHOW: u32 = 5;

/// The instance handle of this task, set before `w16_main` runs.
pub static mut HINST: u32 = 0;

// Windows writes the local heap, atom table and stack bookkeeping into the first 16 bytes of the automatic data segment, so nothing of
// ours may live there. The linker script places this first.
#[used]
#[unsafe(link_section = ".data.00_reserved")]
static RESERVED: [u8; 16] = [0; 16];

unsafe extern "C" {
	/// Provided by the program: the equivalent of `WinMain`. Returns the exit code.
	fn w16_main(hinst: u32, hprev: u32, show: u32) -> u32;
}

// The task entry point. Windows starts the task here with DS = SS, CX = the stack size and DI = 0, and INITTASK reads those registers,
// so nothing may run before it. It returns the instance handles in DI and SI, the command line in ES:BX and the show state in DX.
//
// Rust addresses the stack through ESP, and on a 16-bit stack segment the high half of ESP is not kept: an interrupt that returns to a
// 16-bit stack restores only SP (a 386 rule, which Windows 3.1 in 386 Enhanced mode meets at every timer tick), so a dirty high half
// can turn up between any two instructions. A 32-bit stack segment gets all of ESP back. So the entry code makes a 32-bit alias of the
// stack segment through DPMI and Rust runs on it; the import stubs switch to the real 16-bit stack for each call into Windows, and
// `export_proc!` switches to the alias when Windows calls in. Windows never runs on the alias. If DPMI refuses, both are the real one.
global_asm!(
	".code16",
	".section .data.w16stack,\"aw\"",
	".globl __w16_ss16",
	".globl __w16_ss32",
	"__w16_ss16: .word 0",
	"__w16_ss32: .word 0",
	".section .text.start,\"ax\"",
	".globl _start",
	"_start:",
	".byte 0x9a",
	".globl __w16imp_KERNEL_91_InitTask",
	"__w16imp_KERNEL_91_InitTask:",
	".word 0, 0",
	"or ax, ax",
	"jz 2f",
	"xor bp, bp",
	"movzx esp, sp",
	"push ds",
	"pop es",
	"cld",
	"movzx edx, dx",
	"movzx eax, si",
	"movzx ecx, di",
	"push edx",
	"push eax",
	"push ecx",
	"mov [__w16_ss16], ss",
	"mov [__w16_ss32], ss",
	// DPMI 000Ah: an alias of the data segment; 0009h: the same access rights with the big (B) bit; 0008h: a 64 KB limit.
	"mov bx, ds",
	"mov ax, 0x000a",
	"int 0x31",
	"jc 3f",
	"mov bx, ax",
	"lar cx, bx",
	"mov cl, ch",
	"mov ch, 0x40",
	"mov ax, 0x0009",
	"int 0x31",
	"jc 4f",
	"xor cx, cx",
	"mov dx, 0xffff",
	"mov ax, 0x0008",
	"int 0x31",
	"jc 4f",
	"mov [__w16_ss32], bx",
	"mov ss, bx",
	"movzx esp, sp",
	"jmp 3f",
	"4:",
	"mov ax, 0x0001",
	"int 0x31",
	"3:",
	"push ds",
	"pop es",
	".byte 0x66, 0xe8",
	".long {entry} - . - 4",
	"push eax",
	".byte 0x66, 0xe8",
	".long {exit} - . - 4",
	"2:",
	"mov ah, 0x4c",
	"int 0x21",
	// Never reached: the reference keeps `w16_shadow_overflow` linked, so the packer can send a full shadow stack there.
	".byte 0x66, 0xe9",
	".long {overflow} - . - 4",
	entry = sym w16_entry,
	exit = sym w16_exit,
	overflow = sym w16_shadow_overflow,
);

// Keeps the 32-bit alias over the data segment. Windows may move the data segment in memory during any call (compacting memory, growing
// the local heap), and updates only its own selector, so after each call into Windows the import stubs, and `export_proc!` when Windows
// calls in, read the data segment's base (DPMI 0006h) and move the alias to it (0007h) when it changed. It runs on the 16-bit stack and
// keeps every register.
global_asm!(
	".code16",
	".section .data.w16stack,\"aw\"",
	"__w16_base: .long 0",
	".globl __w16_moves",
	"__w16_moves: .word 0",
	".section .text.w16fixed.sync,\"ax\"",
	".globl w16_sync_alias",
	"w16_sync_alias:",
	"push eax",
	"push ebx",
	"push ecx",
	"push edx",
	"mov bx, word ptr [__w16_ss32]",
	"cmp bx, word ptr [__w16_ss16]",
	"je 2f",
	"mov bx, ds",
	"mov ax, 0x0006",
	"int 0x31",
	"jc 2f",
	"cmp dx, word ptr [__w16_base]",
	"jne 3f",
	"cmp cx, word ptr [__w16_base+2]",
	"je 2f",
	"3:",
	"inc word ptr [__w16_moves]",
	"mov word ptr [__w16_base], dx",
	"mov word ptr [__w16_base+2], cx",
	"mov bx, word ptr [__w16_ss32]",
	"mov ax, 0x0007",
	"int 0x31",
	"2:",
	"pop edx",
	"pop ecx",
	"pop ebx",
	"pop eax",
	".byte 0x66, 0xc3",
);

// Ends the task with the exit code in the low byte of the argument: back to the 16-bit stack, the alias freed, INT 21h function 4Ch.
global_asm!(
	".code16",
	".section .text.w16fixed.exit,\"ax\"",
	".globl w16_exit",
	"w16_exit:",
	"mov dx, word ptr [esp+4]",
	"mov ss, word ptr [__w16_ss16]",
	"movzx esp, sp",
	"mov bx, word ptr [__w16_ss32]",
	"mov ax, ss",
	"cmp bx, ax",
	"je 2f",
	"mov [__w16_ss32], ax",
	"mov ax, 0x0001",
	"int 0x31",
	"2:",
	"mov al, dl",
	"mov ah, 0x4c",
	"int 0x21",
);

unsafe extern "C" {
	/// Ends the task with `code` as its exit code, after leaving the 32-bit stack. Never returns.
	pub fn w16_exit(code: u32) -> !;
	static __w16_ss16: u16;
	static __w16_ss32: u16;
	static __w16_moves: u16;
}

/// How many times the 32-bit stack alias has had to follow the data segment to a new place (the first sync counts as one).
pub fn stack_moves() -> u32 {
	unsafe { __w16_moves as u32 }
}

/// The two stack selectors: the task's 16-bit stack segment, which Windows sees, and the 32-bit alias Rust runs on. They are equal when
/// DPMI could not make the alias.
pub fn stack_selectors() -> (u32, u32) {
	unsafe { (__w16_ss16 as u32, __w16_ss32 as u32) }
}

unsafe extern "C" fn w16_entry(hinst: u32, hprev: u32, show: u32) -> u32 {
	unsafe {
		HINST = hinst;
		WaitEvent(0);
		InitApp(hinst);
		w16_main(hinst, hprev, show)
	}
}

/// Defines an exported far procedure `$name` that calls the Rust function `$f(hwnd, msg, wparam, lparam) -> u32`.
///
/// Windows calls an exported procedure with the Pascal convention on the application's own stack, so SS is the data segment. The
/// wrapper loads DS and ES from SS, saves the registers Windows expects back (and the whole of EBX, ESI, EDI and EBP, which Rust
/// keeps values in), moves to the 32-bit alias of the stack (see `_start`), passes the four arguments as 32-bit values, returns to the
/// stack segment it was called on and returns the result in DX:AX.
#[macro_export]
macro_rules! export_proc {
	($name:ident, $f:path) => {
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
			"mov ax, ss",
			"mov ds, ax",
			"mov es, ax",
			"cld",
			"push ss",
			".byte 0x66, 0xe8",
			".long w16_sync_alias - . - 4",
			"mov ss, word ptr [__w16_ss32]",
			"movzx esp, sp",
			"push dword ptr [bp+8]",
			"movzx eax, word ptr [bp+12]",
			"push eax",
			"movzx eax, word ptr [bp+14]",
			"push eax",
			"movzx eax, word ptr [bp+16]",
			"push eax",
			".byte 0x66, 0xe8",
			".long {f} - . - 4",
			"add sp, 16",
			"pop ss",
			"movzx esp, sp",
			"mov edx, eax",
			"shr edx, 16",
			"pop ebx",
			"pop edi",
			"pop esi",
			"pop es",
			"pop ds",
			"pop ebp",
			".byte 0xca, 0x0a, 0x00",
			f = sym $f,
		);
		unsafe extern "C" {
			pub fn $name();
		}
	};
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
	unsafe {
		MessageBox(
			0,
			c"Rust panic".as_ptr() as u32,
			c"Win16".as_ptr() as u32,
			MB_ICONSTOP,
		);
		w16_exit(3)
	}
}

/// A pointer to a NUL-terminated literal, as the 32-bit value the import stubs take.
#[macro_export]
macro_rules! cs {
	($s:literal) => {
		concat!($s, "\0").as_ptr() as u32
	};
}

// Segment 1's selector. The function lives in segment 1 (the packer pins `.text.w16fixed.*` there), so CS is segment 1's selector
// whichever segment calls it.
global_asm!(
	".code16",
	".section .text.w16fixed.code_selector,\"ax\"",
	".globl w16_code_selector",
	"w16_code_selector:",
	"xor eax, eax",
	"mov ax, cs",
	".byte 0x66, 0xc3",
);

unsafe extern "C" {
	fn w16_code_selector() -> u32;
}

/// The selector of code segment 1, which holds every exported procedure: for turning an exported procedure's offset into a far
/// pointer. It is the same whichever segment the caller is in.
pub fn code_selector() -> u32 {
	unsafe { w16_code_selector() }
}

/// Where a program of several code segments goes when its shadow return stack is full (see docs/packer.md): too many calls through
/// function pointers or cross-segment tail calls are active at once. The packer jumps here far from whichever segment ran out.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.w16fixed.shadow_overflow")]
pub extern "C" fn w16_shadow_overflow() -> ! {
	unsafe {
		asm!("movzx esp, sp", "push ds", "pop es", options(nostack));
		MessageBox(
			0,
			c"Shadow return stack overflow".as_ptr() as u32,
			c"Win16".as_ptr() as u32,
			MB_ICONSTOP,
		);
		w16_exit(4)
	}
}

/// The data segment selector.
pub fn data_selector() -> u32 {
	let ds: u16;
	unsafe { asm!("mov ax, ds", out("ax") ds, options(nomem, nostack, preserves_flags)) };
	ds as u32
}

/// Reads one byte through a far pointer (selector in the high half).
pub fn far_byte(p: u32, i: u32) -> u8 {
	let v: u8;
	unsafe {
		asm!(
			"mov es, dx",
			"mov al, es:[bx]",
			"push ds",
			"pop es",
			in("dx") (p >> 16) as u16,
			in("bx") (p as u16).wrapping_add(i as u16),
			out("al") v,
			options(nostack, preserves_flags),
		);
	}
	v
}

/// Creates a directory with DOS function 39h. Returns 0 on success or the DOS error code.
pub fn dos_mkdir(path: *const u8) -> u16 {
	let ax: u16;
	let cf: u8;
	unsafe {
		asm!(
			"push ebx",
			"push esi",
			"push edi",
			"mov ss, word ptr [__w16_ss16]",
			"movzx esp, sp",
			"int 0x21",
			"setc cl",
			".byte 0x66, 0xe8",
			".long w16_sync_alias - . - 4",
			"mov ss, word ptr [__w16_ss32]",
			"movzx esp, sp",
			"pop edi",
			"pop esi",
			"pop ebx",
			"push ds",
			"pop es",
			inout("ax") 0x3900u16 => ax,
			in("dx") path as u16,
			out("cl") cf,
			options(nostack),
		);
	}
	if cf == 0 { 0 } else { ax }
}

// A small text buffer for the result log. There is no `core::fmt`, which would cost tens of kilobytes of code.
pub struct Log {
	buf: [u8; 3072],
	len: usize,
}

static mut LOG: Log = Log {
	buf: [0; 3072],
	len: 0,
};

pub fn log() -> &'static mut Log {
	unsafe { &mut *addr_of_mut!(LOG) }
}

impl Log {
	pub fn s(&mut self, t: &str) -> &mut Self {
		for b in t.bytes() {
			if self.len < self.buf.len() {
				self.buf[self.len] = b;
				self.len += 1;
			}
		}
		self
	}
	pub fn nl(&mut self) -> &mut Self {
		self.s("\r\n")
	}
	pub fn dec(&mut self, mut v: u32) -> &mut Self {
		let mut tmp = [0u8; 10];
		let mut n = 0;
		loop {
			tmp[n] = b'0' + (v % 10) as u8;
			v /= 10;
			n += 1;
			if v == 0 {
				break;
			}
		}
		while n > 0 {
			n -= 1;
			if self.len < self.buf.len() {
				self.buf[self.len] = tmp[n];
				self.len += 1;
			}
		}
		self
	}
	pub fn hex(&mut self, v: u32, digits: u32) -> &mut Self {
		let mut i = digits;
		while i > 0 {
			i -= 1;
			let d = ((v >> (i * 4)) & 0xF) as u8;
			if self.len < self.buf.len() {
				self.buf[self.len] = if d < 10 { b'0' + d } else { b'A' + d - 10 };
				self.len += 1;
			}
		}
		self
	}
	pub fn bytes(&self) -> &[u8] {
		&self.buf[..self.len]
	}
}

pub const HFILE_ERROR: u32 = 0xFFFF;

/// Writes `data` to a new file. Returns false when the file cannot be created or written.
pub fn write_file(path: *const u8, data: &[u8]) -> bool {
	unsafe {
		let h = _lcreat(path as u32, 0);
		if h == HFILE_ERROR {
			return false;
		}
		let n = _lwrite(h, data.as_ptr() as u32, data.len() as u32);
		_lclose(h);
		n as usize == data.len()
	}
}

/// MAKEINTRESOURCE for the stubs that take a far name pointer: selector 0, the integer in the offset.
pub const fn int_resource(id: u32) -> u32 {
	id & 0xFFFF
}

/// Packs a far pointer from a selector and an offset.
pub fn far(sel: u32, off: u32) -> u32 {
	(sel << 16) | (off & 0xFFFF)
}

// 16-bit Windows structures, packed as the API lays them out.
#[repr(C, packed)]
pub struct WndClass {
	pub style: u16,
	pub wndproc: u32,
	pub cls_extra: u16,
	pub wnd_extra: u16,
	pub instance: u16,
	pub icon: u16,
	pub cursor: u16,
	pub background: u16,
	pub menu: u32,
	pub class_name: u32,
}

#[repr(C, packed)]
pub struct Msg {
	pub hwnd: u16,
	pub message: u16,
	pub wparam: u16,
	pub lparam: u32,
	pub time: u32,
	pub pt: [i16; 2],
}

#[repr(C, packed)]
pub struct PaintStruct {
	pub hdc: u16,
	pub erase: u16,
	pub rect: [i16; 4],
	pub restore: u16,
	pub inc_update: u16,
	pub reserved: [u8; 16],
}
