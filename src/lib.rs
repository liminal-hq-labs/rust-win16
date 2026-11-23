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
global_asm!(
	".code16",
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
	".byte 0x66, 0xe8",
	".long {entry} - . - 4",
	"2:",
	"mov ah, 0x4c",
	"int 0x21",
	entry = sym w16_entry,
);

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
/// keeps values in), clears the high half of ESP, passes the four arguments as 32-bit values and returns the result in DX:AX.
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
		asm!("mov ax, 0x4c03", "int 0x21", options(noreturn));
	}
}

/// A pointer to a NUL-terminated literal, as the 32-bit value the import stubs take.
#[macro_export]
macro_rules! cs {
	($s:literal) => {
		concat!($s, "\0").as_ptr() as u32
	};
}

/// The code segment selector, for turning an exported procedure's offset into a far pointer.
pub fn code_selector() -> u32 {
	let cs: u16;
	unsafe { asm!("mov ax, cs", out("ax") cs, options(nomem, nostack, preserves_flags)) };
	cs as u32
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
			"int 0x21",
			"setc cl",
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
