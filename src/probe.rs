// Register-preservation probes: load EBX, ESI, EDI and EBP with patterns whose high halves are non-zero, make one Windows call and report
// what came back.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// Rust code on this target keeps live values in the 32-bit registers, and a 16-bit callee need not preserve their high halves. The
// import stubs and exported-procedure wrappers save and restore them for that reason; these probes call the APIs raw, to find out whether
// the guest would have needed it.
use core::arch::global_asm;

use crate::Log;

pub const PAT_EBX: u32 = 0x1111_BE01;
pub const PAT_ESI: u32 = 0x2222_BE02;
pub const PAT_EDI: u32 = 0x3333_BE03;
pub const PAT_EBP: u32 = 0x4444_BE04;

/// Scratch message buffer for the PeekMessage probe (a 16-bit MSG is 18 bytes).
#[unsafe(no_mangle)]
pub static mut PROBE_MSG: [u8; 32] = [0; 32];

macro_rules! probe {
	($name:ident, $module:literal, $ordinal:literal, [$($line:literal),*]) => {
		global_asm!(
			".code16",
			concat!(".section .text.", stringify!($name), ",\"ax\""),
			concat!(".globl ", stringify!($name)),
			concat!(stringify!($name), ":"),
			"push ebx",
			"push esi",
			"push edi",
			"push ebp",
			"mov ebx, 0x1111BE01",
			"mov esi, 0x2222BE02",
			"mov edi, 0x3333BE03",
			"mov ebp, 0x4444BE04",
			$($line,)*
			".byte 0x9a",
			concat!(".globl __w16imp_", $module, "_", $ordinal, "_", stringify!($name)),
			concat!("__w16imp_", $module, "_", $ordinal, "_", stringify!($name), ":"),
			".word 0, 0",
			"mov eax, esp",
			"movzx esp, sp",
			"push ebp",
			"push edi",
			"push esi",
			"push ebx",
			"mov bp, sp",
			"mov di, [bp+36]",
			"mov edx, [bp]",
			"mov [di], edx",
			"mov edx, [bp+4]",
			"mov [di+4], edx",
			"mov edx, [bp+8]",
			"mov [di+8], edx",
			"mov edx, [bp+12]",
			"mov [di+12], edx",
			"mov [di+16], eax",
			"add sp, 16",
			"push ds",
			"pop es",
			"pop ebp",
			"pop edi",
			"pop esi",
			"pop ebx",
			".byte 0x66, 0xc3",
		);
	};
}

probe!(probe_getversion, "KERNEL", "3", []);
probe!(probe_yield, "KERNEL", "29", []);
probe!(
	probe_globalalloc,
	"KERNEL",
	"15",
	["push 2", "push 0", "push 64"]
);
probe!(
	probe_peekmessage,
	"USER",
	"109",
	[
		"push ds",
		"mov ax, offset PROBE_MSG",
		"push ax",
		"push 0",
		"push 0",
		"push 0",
		"push 1"
	]
);

unsafe extern "C" {
	fn probe_getversion(out: *mut [u32; 5]);
	fn probe_yield(out: *mut [u32; 5]);
	fn probe_globalalloc(out: *mut [u32; 5]);
	fn probe_peekmessage(out: *mut [u32; 5]);
}

fn report(log: &mut Log, name: &str, r: &[u32; 5]) -> bool {
	let want = [PAT_EBX, PAT_ESI, PAT_EDI, PAT_EBP];
	let mut ok = true;
	log.s("probe ")
		.s(name)
		.s(": EBX=")
		.hex(r[0], 8)
		.s(" ESI=")
		.hex(r[1], 8)
		.s(" EDI=")
		.hex(r[2], 8)
		.s(" EBP=")
		.hex(r[3], 8);
	log.s(" ESP=").hex(r[4], 8);
	for i in 0..4 {
		if r[i] != want[i] {
			ok = false;
		}
	}
	log.s(if ok { " PRESERVED" } else { " CLOBBERED" }).nl();
	ok
}

/// Runs every probe and logs the results. Returns true when every register survived every call.
pub fn run_all(log: &mut Log, tag: &str) -> bool {
	let mut r = [0u32; 5];
	let mut all = true;
	log.s("-- register probes (").s(tag).s(")").nl();
	unsafe {
		probe_getversion(&mut r);
		all &= report(log, "GetVersion", &r);
		probe_globalalloc(&mut r);
		all &= report(log, "GlobalAlloc", &r);
		probe_peekmessage(&mut r);
		all &= report(log, "PeekMessage", &r);
		probe_yield(&mut r);
		all &= report(log, "Yield", &r);
	}
	all
}
