// Big segments: more than 64 KB of code in several code segments, calling across all of them directly, through function pointers,
// through `dyn` trait objects and through a jump table, and checking every result against a second implementation.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// `work::<K>` is one function per K (300 of them, a few hundred bytes each): the bulk of the code, which the packer spreads over the
// automatically filled segments. Each calls `dispatch`, a dense `match` that tail-calls the next `work`, and every fourth calls another
// through `TABLE`, so every segment calls every other. `work_ref` computes the same values with K as a run-time value, in one function.
// `cold` sits in a load-on-call, discardable segment (see segments.json); `pong` is placed by an attribute.
//
// Command line: `/quiet` skips the closing message box, `/exit` ends Windows when done (for unattended runs). The log is written next to
// the program as BIGSEG.LOG and ends with `RESULT: PASS` or `RESULT: FAIL`.
#![no_std]
#![no_main]

use core::arch::asm;
use core::ptr::addr_of_mut;
use w16::*;

const N: u32 = 300;

type WorkFn = fn(u32, u32) -> u32;

/// Mixes `v` with constants derived from `k`. Inlined into every `work::<K>`, where `k` is a constant, and into `work_ref`, where it is
/// not, so the two compute the same thing through different code.
#[inline(always)]
fn mix(mut v: u32, k: u32) -> u32 {
	let c = k.wrapping_mul(0x9E37_79B9).wrapping_add(0x7F4A_7C15);
	v ^= c;
	v = v.wrapping_mul(c | 1);
	v = v.rotate_left(k % 29 + 1);
	v = v.wrapping_add(c.rotate_right(7));
	v ^= v >> (k % 13 + 3);
	v = v.wrapping_mul(0x85EB_CA6B ^ k);
	v = v.wrapping_sub(c >> 3);
	v ^= v << (k % 7 + 1);
	v = v.wrapping_mul(0xC2B2_AE35 ^ k.wrapping_mul(3));
	v = v.rotate_right(k % 17 + 2);
	v ^= c.wrapping_mul(0x27D4_EB2F);
	v = v.wrapping_add(0x1656_67B1 ^ k.wrapping_mul(5));
	v ^= v >> 15;
	v = v.wrapping_mul(0x2545_F491 ^ k.wrapping_mul(7));
	v = v.rotate_left(k % 11 + 3);
	v ^= 0xDEAD_BEEF ^ k.wrapping_mul(0x0101_0101);
	v = v.wrapping_mul(0x9E37_79B1 ^ k.wrapping_mul(11));
	v ^= v >> (k % 9 + 5);
	v = v.wrapping_add(c.rotate_left(k % 5 + 9));
	v
}

#[inline(never)]
fn work<const K: u32>(x: u32, d: u32) -> u32 {
	let mut v = mix(x, K);
	if d > 0 {
		v = v.wrapping_add(dispatch((K * 7 + 3) % N, v, d - 1));
		if K % 4 == 0 {
			let f = core::hint::black_box(&TABLE)[((K * 13 + 5) % N) as usize];
			v ^= f(v, 0);
		}
	}
	v
}

/// The same values as `work::<k>`, computed with `k` known only at run time.
#[inline(never)]
fn work_ref(k: u32, x: u32, d: u32) -> u32 {
	let mut v = mix(x, k);
	if d > 0 {
		v = v.wrapping_add(work_ref((k * 7 + 3) % N, v, d - 1));
		if k % 4 == 0 {
			v ^= work_ref((k * 13 + 5) % N, v, 0);
		}
	}
	v
}

// Calls `$m!(n)` for every n below N, from three decimal digits.
macro_rules! each {
	($m:ident) => {
		each!(@h $m [0 1 2]);
	};
	(@h $m:ident [$($h:literal)*]) => {
		$( each!(@t $m $h [0 1 2 3 4 5 6 7 8 9]); )*
	};
	(@t $m:ident $h:literal [$($t:literal)*]) => {
		$( each!(@o $m $h $t [0 1 2 3 4 5 6 7 8 9]); )*
	};
	(@o $m:ident $h:literal $t:literal [$($o:literal)*]) => {
		$( $m!($h * 100 + $t * 10 + $o); )*
	};
}

/// Calls `work::<k>`. Each arm is a tail call, so the jumps leave this segment for whichever segment holds that `work`.
#[inline(never)]
fn dispatch(k: u32, x: u32, d: u32) -> u32 {
	macro_rules! arm {
		($n:expr) => {
			if k == $n {
				return work::<{ $n }>(x, d);
			}
		};
	}
	each!(arm);
	0
}

/// Every `work::<K>` as a function pointer: 300 code addresses in the data segment, each redirected to a slot by the packer.
static TABLE: [WorkFn; N as usize] = {
	let mut t = [work::<0> as WorkFn; N as usize];
	macro_rules! set {
		($n:expr) => {
			t[$n] = work::<{ $n }>;
		};
	}
	each!(set);
	t
};

/// A dense `match` that LLVM turns into a jump table: an indirect jump through code offsets stored in the data segment.
#[inline(never)]
fn jump_table(op: u32, a: u32, b: u32) -> u32 {
	match op {
		0 => a.wrapping_add(b),
		1 => a.wrapping_sub(b),
		2 => a ^ b,
		3 => a.wrapping_mul(b),
		4 => a.rotate_left(b & 31),
		5 => a | b,
		6 => a & !b,
		7 => a.wrapping_add(b.wrapping_mul(3)),
		8 => (a >> 3) ^ b,
		9 => a.wrapping_mul(7).wrapping_add(b),
		_ => 0xFFFF_FFFF,
	}
}

fn jump_table_ref(op: u32, a: u32, b: u32) -> u32 {
	let v = [
		a.wrapping_add(b),
		a.wrapping_sub(b),
		a ^ b,
		a.wrapping_mul(b),
		a.rotate_left(b & 31),
		a | b,
		a & !b,
		a.wrapping_add(b.wrapping_mul(3)),
		(a >> 3) ^ b,
		a.wrapping_mul(7).wrapping_add(b),
	];
	if op < 10 { v[op as usize] } else { 0xFFFF_FFFF }
}

trait Shape {
	fn area(&self) -> u32;
	fn grow(&self, by: u32) -> u32;
}

struct Square(u32);
struct Rect(u32, u32);
struct Tri(u32, u32);

impl Shape for Square {
	fn area(&self) -> u32 {
		self.0 * self.0
	}
	fn grow(&self, by: u32) -> u32 {
		(self.0 + by) * (self.0 + by)
	}
}

impl Shape for Rect {
	fn area(&self) -> u32 {
		self.0 * self.1
	}
	fn grow(&self, by: u32) -> u32 {
		(self.0 + by) * (self.1 + by)
	}
}

impl Shape for Tri {
	fn area(&self) -> u32 {
		self.0 * self.1 / 2
	}
	fn grow(&self, by: u32) -> u32 {
		(self.0 + by) * (self.1 + by) / 2
	}
}

static SHAPES: [&(dyn Shape + Sync); 3] = [&Square(7), &Rect(3, 5), &Tri(4, 9)];

/// Code that runs rarely, in segment 3: loaded on the first call and discardable, so Windows can drop it and load it again.
mod cold {
	use super::*;

	#[inline(never)]
	pub fn shapes_total(by: u32) -> u32 {
		let mut t = 0u32;
		for s in core::hint::black_box(&SHAPES).iter() {
			t = t
				.wrapping_add(s.area())
				.wrapping_mul(3)
				.wrapping_add(s.grow(by));
		}
		t
	}

	#[inline(never)]
	pub fn sum_table(x: u32) -> u32 {
		let mut t = 0u32;
		let mut i = 0;
		while i < N as usize {
			let f = core::hint::black_box(&TABLE)[i];
			t = t.wrapping_add(f(x ^ i as u32, 0));
			i += 13;
		}
		t
	}
}

/// Mutual recursion across segments: `pong` is in segment 4 by its section name, `ping` wherever the automatic fill puts it. Neither
/// call is a tail call.
#[inline(never)]
fn ping(n: u32) -> u32 {
	if n == 0 {
		0
	} else {
		pong(core::hint::black_box(n - 1)).wrapping_add(3)
	}
}

#[inline(never)]
#[unsafe(link_section = ".text.seg4.pong")]
fn pong(n: u32) -> u32 {
	if n == 0 {
		1
	} else {
		ping(core::hint::black_box(n - 1)).wrapping_add(5)
	}
}

fn ping_ref(n: u32) -> u32 {
	// The innermost call returns 0 (ping) or 1 (pong); on the way back each ping adds 3 and each pong 5.
	let mut acc = if n % 2 == 0 { 0 } else { 1 };
	let mut level = 0;
	while level < n {
		acc += if level % 2 == 0 { 3 } else { 5 };
		level += 1;
	}
	acc
}

/// Tail calls into segment 4: a plain one, a conditional one and a mutual recursion whose every step is a tail call across segments.
#[inline(never)]
fn hop(x: u32) -> u32 {
	tail_target(x.wrapping_mul(3))
}

#[inline(never)]
fn hop_if(x: u32) -> u32 {
	if x & 1 != 0 {
		tail_target(x)
	} else {
		x.wrapping_add(1)
	}
}

#[inline(never)]
#[unsafe(link_section = ".text.seg4.tail_target")]
fn tail_target(x: u32) -> u32 {
	x ^ 0x5A5A_A5A5
}

#[inline(never)]
fn tick(n: u32, acc: u32) -> u32 {
	if n == 0 {
		acc
	} else {
		tock(n - 1, acc.wrapping_add(3))
	}
}

#[inline(never)]
#[unsafe(link_section = ".text.seg4.tock")]
fn tock(n: u32, acc: u32) -> u32 {
	if n == 0 {
		acc ^ 1
	} else {
		tick(n - 1, acc.wrapping_mul(5))
	}
}

fn tick_ref(mut n: u32) -> u32 {
	let mut acc = 0u32;
	let mut in_tick = true;
	loop {
		if n == 0 {
			return if in_tick { acc } else { acc ^ 1 };
		}
		acc = if in_tick {
			acc.wrapping_add(3)
		} else {
			acc.wrapping_mul(5)
		};
		n -= 1;
		in_tick = !in_tick;
	}
}

/// Nested calls through function pointers into segment 4 and back: each level holds one shadow stack entry.
static NEST: [fn(u32) -> u32; 2] = [nest_a, nest_b];

#[inline(never)]
fn nest_a(n: u32) -> u32 {
	if n == 0 {
		7
	} else {
		core::hint::black_box(&NEST)[1](n - 1)
			.wrapping_mul(3)
			.wrapping_add(1)
	}
}

#[inline(never)]
#[unsafe(link_section = ".text.seg4.nest_b")]
fn nest_b(n: u32) -> u32 {
	if n == 0 {
		11
	} else {
		core::hint::black_box(&NEST)[0](n - 1).wrapping_add(n)
	}
}

fn nest_ref(n: u32) -> u32 {
	if n == 0 {
		return 7;
	}
	let mut v: u32 = if n % 2 == 0 { 7 } else { 11 };
	// Level i (0 at the top) is nest_a when i is even, nest_b otherwise; it is called with n - i.
	let mut i = n;
	while i > 0 {
		i -= 1;
		v = if i % 2 == 0 {
			v.wrapping_mul(3).wrapping_add(1)
		} else {
			v.wrapping_add(n - i)
		};
	}
	v
}

/// Spins for `rounds` iterations without calling Windows and counts how often the high half of ESP is not zero, on the 16-bit stack
/// segment (`on16`) or on the 32-bit alias Rust runs on. An interrupt that returns to a 16-bit stack restores only SP, so on the
/// 16-bit stack the count shows the timer ticks; on the 32-bit alias it should be zero.
fn esp_watch(rounds: u32, on16: bool) -> (u32, u32) {
	let dirty: u32;
	let seen: u32;
	unsafe {
		asm!(
			"push ebx",
			"test al, al",
			"jz 4f",
			"mov ss, word ptr [__w16_ss16]",
			"movzx esp, sp",
			"4:",
			"xor ebx, ebx",
			"xor edx, edx",
			"2:",
			"mov eax, esp",
			"shr eax, 16",
			"jz 3f",
			"inc ebx",
			"mov edx, eax",
			"movzx esp, sp",
			"3:",
			"dec ecx",
			"jnz 2b",
			"mov eax, ebx",
			"mov ss, word ptr [__w16_ss32]",
			"movzx esp, sp",
			"pop ebx",
			inout("ecx") rounds => _,
			inout("eax") on16 as u32 => dirty,
			out("edx") seen,
		);
	}
	(dirty, seen)
}

// Segment 3's selector, from a selector fixup the packer writes, and whether the segment is in memory (the present bit that LAR reads).
core::arch::global_asm!(
	".code16",
	".section .text.w16fixed.seg3_present,\"ax\"",
	".globl seg3_present",
	"seg3_present:",
	".byte 0xb8",
	".globl __w16sel_seg3_present",
	"__w16sel_seg3_present:",
	".word 0xffff",
	"xor ecx, ecx",
	"lar cx, ax",
	"movzx eax, ax",
	"shl eax, 16",
	"shr cx, 15",
	"or ax, cx",
	".byte 0x66, 0xc3",
);

unsafe extern "C" {
	/// Segment 3's selector in the high half, and 1 in the low half when the segment is present.
	fn seg3_present() -> u32;
	/// Calls `GlobalCompact(-1)` through its stub with EBP set to `frame`.
	fn compact_with_bp(frame: u32) -> u32;
}

// Calls GlobalCompact(-1) with EBP pointing where the caller says, the way Rust code may leave EBP holding a pointer into its own stack.
core::arch::global_asm!(
	".code16",
	".section .text.w16fixed.compact_with_bp,\"ax\"",
	".globl compact_with_bp",
	"compact_with_bp:",
	"push ebp",
	"mov ebp, dword ptr [esp+8]",
	".byte 0x66, 0x6a, 0xff",
	".byte 0x66, 0xe8",
	".long GlobalCompact - . - 4",
	"add sp, 4",
	"pop ebp",
	".byte 0x66, 0xc3",
);

static mut MSG: Msg = Msg {
	hwnd: 0,
	message: 0,
	wparam: 0,
	lparam: 0,
	time: 0,
	pt: [0; 2],
};

export_proc!(BigsegWndProc, bigseg_wndproc);

static mut CLASS: WndClass = WndClass {
	style: 0,
	wndproc: 0,
	cls_extra: 0,
	wnd_extra: 0,
	instance: 0,
	icon: 0,
	cursor: 0,
	background: 0,
	menu: 0,
	class_name: 0,
};
static mut HWND: u32 = 0;
static mut CLASS_ATOM: u32 = 0;
/// Bytes the window procedure found changed in its own stack buffer, over every WM_USER it handled.
static mut INNER_CHANGED: u32 = 0;
static mut INNER_RUNS: u32 = 0;

extern "C" fn bigseg_wndproc(hwnd: u32, msg: u32, wparam: u32, lparam: u32) -> u32 {
	if msg != WM_USER {
		return unsafe { DefWindowProc(hwnd, msg, wparam, lparam) };
	}
	// A stack buffer in a callback, live across calls that yield and compact, while the caller's buffer is live below it.
	let mut buf = [0u8; 512];
	for (i, b) in buf.iter_mut().enumerate() {
		*b = (i as u8) ^ 0x3C;
	}
	let p = core::hint::black_box(buf.as_mut_ptr());
	unsafe {
		Yield();
		GlobalCompact(0xFFFF_FFFF);
		cold::shapes_total(2);
		let got = core::slice::from_raw_parts(core::hint::black_box(p), 512);
		for i in 0..512 {
			if got[i] != (i as u8) ^ 0x3C {
				INNER_CHANGED += 1;
			}
		}
		INNER_RUNS += 1;
	}
	0
}

/// A hidden window, so that the stack-buffer check also runs through `DispatchMessage` and `export_proc!`.
fn make_window(hinst: u32) -> u32 {
	unsafe {
		let c = addr_of_mut!(CLASS);
		(*c).wndproc = far(code_selector(), BigsegWndProc as *const () as usize as u32);
		(*c).instance = hinst as u16;
		(*c).class_name = far(data_selector(), cs!("BigsegWin"));
		CLASS_ATOM = RegisterClass(c as u32);
		HWND = CreateWindow(
			cs!("BigsegWin"),
			cs!("Big segments"),
			0,
			0,
			0,
			10,
			10,
			0,
			0,
			hinst,
			0,
		);
		HWND
	}
}

/// Fills a 512-byte buffer on the stack with a pattern, makes calls into Windows that yield, discard and move memory, and logs every byte
/// that changed. With `provoke`, the buffer holds a frame that looks like a far return into segment 3, and EBP points at it during
/// `GlobalCompact`, as it might if Rust kept a pointer into the buffer in EBP.
#[inline(never)]
fn stack_buffer(log: &mut Log, name: &str, provoke: bool) -> bool {
	let mut buf = [0u8; 512];
	for (i, b) in buf.iter_mut().enumerate() {
		*b = (i as u8) ^ 0xA5;
	}
	// Windows marks a far frame by an odd link to it: EBP will be the frame's address plus one, so the frame must sit at an even address.
	let frame = if (buf.as_ptr() as usize + 0x100) & 1 == 0 {
		0x100
	} else {
		0x101
	};
	if provoke {
		let seg3 = (unsafe { seg3_present() } >> 16) as u16;
		buf[frame..frame + 2].copy_from_slice(&0u16.to_le_bytes()); // the end of the chain
		buf[frame + 2..frame + 4].copy_from_slice(&0x0c40u16.to_le_bytes()); // a far return into segment 3
		buf[frame + 4..frame + 6].copy_from_slice(&seg3.to_le_bytes());
	}
	let want = buf;
	let p = core::hint::black_box(buf.as_mut_ptr());
	unsafe {
		cold::shapes_total(1);
		let m = addr_of_mut!(MSG) as u32;
		PeekMessage(m, 0, 0, 0, PM_REMOVE);
		Yield();
		if provoke {
			compact_with_bp(p as u32 + frame as u32 + 1);
		} else {
			GlobalCompact(0xFFFF_FFFF);
		}
		PeekMessage(m, 0, 0, 0, PM_REMOVE);
		if HWND != 0 {
			PostMessage(HWND, WM_USER, 0, 0);
			while PeekMessage(m, 0, 0, 0, PM_REMOVE) != 0 {
				DispatchMessage(m);
			}
		}
	}
	let got = unsafe { core::slice::from_raw_parts(core::hint::black_box(p), 512) };
	let mut changed = 0;
	for i in 0..512 {
		if got[i] != want[i] {
			if changed < 8 {
				log.s("  +")
					.hex(i as u32, 3)
					.s(": ")
					.hex(want[i] as u32, 2)
					.s(" -> ")
					.hex(got[i] as u32, 2)
					.nl();
			}
			changed += 1;
		}
	}
	check(log, name, changed, 0)
}

fn check(log: &mut Log, name: &str, got: u32, want: u32) -> bool {
	log.s(name).s(": ").hex(got, 8);
	let ok = got == want;
	if ok {
		log.s(" ok").nl();
	} else {
		log.s(" want ").hex(want, 8).s(" FAIL").nl();
	}
	// Written after every check, so a run that dies shows how far it got.
	save_log(log, unsafe { HINST });
	ok
}

/// The PSP command tail, as bytes, through the far pointer GetCurrentPDB gives.
fn command_has(flag: &[u8]) -> bool {
	let psp = unsafe { GetCurrentPDB() };
	let p = far(psp, 0x80);
	let n = far_byte(p, 0) as u32;
	let mut i = 1;
	while i + flag.len() as u32 <= n + 1 {
		let mut j = 0;
		while j < flag.len() && far_byte(p, i + j as u32).to_ascii_lowercase() == flag[j] {
			j += 1;
		}
		if j == flag.len() {
			return true;
		}
		i += 1;
	}
	false
}

static mut PATH: [u8; 144] = [0; 144];

fn save_log(log: &Log, hinst: u32) {
	unsafe {
		let path = &mut *addr_of_mut!(PATH);
		let n = GetModuleFileName(hinst, path.as_mut_ptr() as u32, 130) as usize;
		let mut cut = n;
		while cut > 0 && path[cut - 1] != b'\\' && path[cut - 1] != b':' {
			cut -= 1;
		}
		for (i, b) in b"BIGSEG.LOG\0".iter().enumerate() {
			path[cut + i] = *b;
		}
		write_file(path.as_ptr(), log.bytes());
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn w16_main(hinst: u32, _hprev: u32, _show: u32) -> u32 {
	let log = log();
	let quiet = command_has(b"/quiet") || command_has(b"/exit");
	let mut ok = true;
	log.s("Rust Win16 big segments").nl();
	log.s("windows: ")
		.hex(unsafe { GetVersion() }, 8)
		.s(" flags ")
		.hex(unsafe { GetWinFlags() }, 8)
		.nl();
	let (ss16, ss32) = stack_selectors();
	log.s("code selector ")
		.hex(code_selector(), 4)
		.s(", stack ")
		.hex(ss16, 4)
		.s(", 32-bit stack ")
		.hex(ss32, 4)
		.nl();

	// Direct calls and tail calls across every segment, checked against the one-function reference.
	let mut k = 0;
	while k < N {
		let x = 0x1234_5678 ^ k.wrapping_mul(0x0100_0193);
		let got = dispatch(k, x, 6);
		let want = work_ref(k, x, 6);
		if got != want {
			log.s("work ")
				.dec(k)
				.s(": ")
				.hex(got, 8)
				.s(" want ")
				.hex(want, 8)
				.s(" FAIL")
				.nl();
			ok = false;
		}
		k += 1;
	}
	log.s("work: ")
		.dec(N)
		.s(" functions, depth 6, compared")
		.nl();

	// Function pointers called from this segment, and from the cold segment.
	let mut t = 0u32;
	let mut i = 0;
	while i < N as usize {
		let f = core::hint::black_box(&TABLE)[i];
		t = t.wrapping_add(f(0xABCD ^ i as u32, 2));
		i += 7;
	}
	let mut w = 0u32;
	let mut i = 0;
	while i < N as usize {
		w = w.wrapping_add(work_ref(i as u32, 0xABCD ^ i as u32, 2));
		i += 7;
	}
	ok &= check(log, "pointers from segment 1", t, w);

	let mut want_sum = 0u32;
	let mut i = 0;
	while i < N as usize {
		want_sum = want_sum.wrapping_add(work_ref(i as u32, 0x5A5A ^ i as u32, 0));
		i += 13;
	}
	ok &= check(
		log,
		"pointers from the cold segment",
		cold::sum_table(0x5A5A),
		want_sum,
	);

	// `dyn` calls through vtables, from the cold segment.
	let mut want_shapes = 0u32;
	for (a, g) in [(49u32, 144u32), (15, 80), (18, 63)] {
		want_shapes = want_shapes.wrapping_add(a).wrapping_mul(3).wrapping_add(g);
	}
	ok &= check(log, "dyn shapes", cold::shapes_total(5), want_shapes);

	// A jump table.
	let mut jt = 0u32;
	let mut jr = 0u32;
	for op in 0..12u32 {
		jt = jt.wrapping_mul(31).wrapping_add(jump_table(
			core::hint::black_box(op),
			0x1357_9BDF,
			op + 3,
		));
		jr = jr
			.wrapping_mul(31)
			.wrapping_add(jump_table_ref(op, 0x1357_9BDF, op + 3));
	}
	ok &= check(log, "jump table", jt, jr);

	// Deep recursion across two segments through the call veneers, which keep no state.
	ok &= check(log, "ping 600 deep", ping(600), ping_ref(600));

	// Tail calls across segments, and deep chains of them, which take one shadow stack entry however long they are.
	ok &= check(
		log,
		"tail call",
		hop(core::hint::black_box(0x0102_0304)),
		0x0306_090C ^ 0x5A5A_A5A5,
	);
	ok &= check(
		log,
		"conditional tail call",
		hop_if(core::hint::black_box(0x11)),
		0x11 ^ 0x5A5A_A5A5,
	);
	ok &= check(
		log,
		"conditional, not taken",
		hop_if(core::hint::black_box(0x10)),
		0x11,
	);
	ok &= check(
		log,
		"tail calls 2000 deep",
		tick(core::hint::black_box(2000), 0),
		tick_ref(2000),
	);
	ok &= check(
		log,
		"pointer calls 200 deep",
		nest_a(core::hint::black_box(200)),
		nest_ref(200),
	);

	// Throw away every discardable segment, then call into the cold one again.
	let before = unsafe { seg3_present() };
	let freed = unsafe { GlobalCompact(0xFFFF_FFFF) };
	let after = unsafe { seg3_present() };
	log.s("GlobalCompact: ")
		.hex(freed, 8)
		.s("; segment 3 (")
		.hex(before >> 16, 4)
		.s(") present before ")
		.dec(before & 1);
	log.s(", after ").dec(after & 1).nl();
	ok &= check(
		log,
		"cold segment after a discard",
		cold::shapes_total(5),
		want_shapes,
	);

	ok &= check(
		log,
		"segment 3 present again",
		unsafe { seg3_present() } & 1,
		1,
	);

	let hwnd = make_window(hinst);
	log.s("window: class ")
		.hex(unsafe { CLASS_ATOM }, 4)
		.s(", hwnd ")
		.hex(hwnd, 4)
		.nl();
	let mut plain = true;
	for _ in 0..10 {
		plain &= stack_buffer(log, "stack buffer bytes changed", false);
	}
	ok &= plain;
	log.s("window procedure buffer checks: ")
		.dec(unsafe { INNER_RUNS })
		.nl();
	ok &= check(
		log,
		"window procedure buffer bytes changed",
		unsafe { INNER_CHANGED },
		0,
	);
	ok &= stack_buffer(log, "stack buffer bytes changed, EBP into it", true);

	let (dirty16, seen16) = esp_watch(4_000_000, true);
	log.s("esp watch, 16-bit stack: ")
		.dec(dirty16)
		.s(" dirty of 4000000, last high half ")
		.hex(seen16, 4)
		.nl();
	let (dirty, seen) = esp_watch(4_000_000, false);
	log.s("esp watch, 32-bit stack: ")
		.dec(dirty)
		.s(" dirty of 4000000, last high half ")
		.hex(seen, 4)
		.nl();
	ok &= check(log, "esp dirty on the 32-bit stack", dirty, 0);
	log.s("stack alias moves: ").dec(stack_moves()).nl();

	log.s(if ok { "RESULT: PASS" } else { "RESULT: FAIL" }).nl();
	save_log(log, hinst);
	unsafe {
		if !quiet {
			MessageBox(
				0,
				if ok {
					cs!("All big-segment checks passed")
				} else {
					cs!("A big-segment check failed; see BIGSEG.LOG")
				},
				cs!("Rust Win16 big segments"),
				if ok { MB_ICONINFORMATION } else { MB_ICONSTOP },
			);
		}
		if command_has(b"/exit") {
			ExitWindows(0, 0);
		}
	}
	if ok { 0 } else { 1 }
}
