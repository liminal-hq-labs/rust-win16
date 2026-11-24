// Segment move: tries to make Windows move the data segment while a blocking call is open, and checks that the 32-bit stack alias follows.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// During `MessageBox` or `GetOpenFileName` Windows runs its own modal message loop and calls the program's exported procedures (a timer
// callback, a common dialog hook) before the import stub returns. The callback here forces memory movement with Windows calls that may
// move the automatic data segment (moveable `GlobalAlloc` blocks and `GlobalCompact`, local heap growth with `LocalAlloc` and
// `LocalReAlloc`, big allocations, and with `/dsrealloc` a `GlobalReAlloc` of the data segment's own handle), then closes the dialog
// itself so that no one has to press a key. After each step it reads the data segment's base and the alias's base through DPMI (0006h),
// counts the bytes that changed in a buffer on the stack and in a table in data, and logs `stack_moves()` (the runtime's count of times
// the alias followed the data segment). The same buffer, kept on the program's own stack across the blocking call, is checked again
// when the call returns, which is where a stale alias would show.
//
// Command line: `/mb` runs the `MessageBox` variant (the default), `/cd` the `GetOpenFileName` variant with an `OFN_ENABLEHOOK` hook,
// `/dsrealloc` adds the `GlobalReAlloc` step, `/exit` ends Windows when done (for unattended runs). The log is written next to the
// program as SEGMOVE.LOG and ends with `RESULT: PASS` or `RESULT: FAIL`, then `MOVES: <n>` (how many times the data segment moved
// during the blocking call, by the runtime's counter) and `EXIT: <code>`. The exit code is also the program's, 0 on a pass. A pass means
// the bytes and the alias were intact; whether Windows actually moved the segment is the `MOVES` line, which the harness reports apart.
#![no_std]
#![no_main]

use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut, read_volatile};
use w16::*;

const GMEM_MOVEABLE: u32 = 2;
const WM_TIMER: u32 = 0x0113;
const OFN_HIDEREADONLY: u32 = 0x0004;
const OFN_NOCHANGEDIR: u32 = 0x0008;
const OFN_ENABLEHOOK: u32 = 0x0020;
const TIMER_MS: u32 = 400;
const HOOK_TIMER: u32 = 77;
const STACK_XOR: u8 = 0x5A;
const DATA_XOR: u8 = 0xC3;
/// The message box title, which `FindWindow` looks for.
fn box_title() -> u32 {
	cs!("SegmoveBox")
}

export_proc!(SegTimer, seg_timer);
export_proc!(SegHook, seg_hook);

/// A table in the data segment, filled with a pattern at start-up and checked after every step.
static mut DATA_PAT: [u8; 256] = [0; 256];

/// Bytes found changed (stack buffer and table), alias mismatches against the data segment, and steps made, over the whole run.
static mut BYTES_BAD: u32 = 0;
static mut ALIAS_BAD: u32 = 0;
static mut STEPS: u32 = 0;
/// How many times the callback or hook ran, and whether the forcing already ran for the open dialog.
static mut CALLS: u32 = 0;
static mut FORCED: bool = false;
/// The dialog's window handle once found, and the timer the message box variant installed.
static mut DLG: u32 = 0;
static mut TIMER_ID: u32 = 0;
/// `stack_moves()` read at the start of the blocking call and again when the callback began.
static mut MOVES_AT_CALL: u32 = 0;
static mut MOVES_AT_CB: u32 = 0;
static mut WITH_DS_REALLOC: bool = false;
/// The control alias (see `dpmi_alias`), and `stack_moves()` when it was made.
static mut CONTROL: u32 = 0;
static mut MOVES_AT_CONTROL: u32 = 0;

#[repr(C, packed)]
struct OpenFileName {
	size: u32,
	owner: u16,
	instance: u16,
	filter: u32,
	custom_filter: u32,
	max_custom: u32,
	filter_index: u32,
	file: u32,
	max_file: u32,
	file_title: u32,
	max_file_title: u32,
	initial_dir: u32,
	title: u32,
	flags: u32,
	file_offset: u16,
	file_ext: u16,
	def_ext: u32,
	cust_data: u32,
	hook: u32,
	template: u32,
}

static mut OFN: OpenFileName = OpenFileName {
	size: 0,
	owner: 0,
	instance: 0,
	filter: 0,
	custom_filter: 0,
	max_custom: 0,
	filter_index: 0,
	file: 0,
	max_file: 0,
	file_title: 0,
	max_file_title: 0,
	initial_dir: 0,
	title: 0,
	flags: 0,
	file_offset: 0,
	file_ext: 0,
	def_ext: 0,
	cust_data: 0,
	hook: 0,
	template: 0,
};
static mut OFN_FILE: [u8; 260] = [0; 260];

/// The base address of a selector, through DPMI 0006h on the real stack (the alias is not synced here, so a stale one stays visible).
fn dpmi_base(sel: u32) -> u32 {
	let hi: u16;
	let lo: u16;
	unsafe {
		asm!(
			"push ebx",
			"mov bx, dx",
			"mov ss, word ptr [__w16_ss16]",
			"movzx esp, sp",
			"mov ax, 0x0006",
			"int 0x31",
			"mov ss, word ptr [__w16_ss32]",
			"movzx esp, sp",
			"pop ebx",
			"push ds",
			"pop es",
			inout("dx") sel as u16 => lo,
			out("cx") hi,
			out("ax") _,
			options(nostack),
		);
	}
	((hi as u32) << 16) | lo as u32
}

/// A second selector over the data segment, made once and never moved: DPMI 0Ah copies the descriptor, base and all. After Windows has
/// moved the data segment it still holds the old base. Returns 0 when DPMI refuses.
fn dpmi_alias(sel: u32) -> u32 {
	let new: u16;
	let cf: u8;
	unsafe {
		asm!(
			"push ebx",
			"mov bx, dx",
			"mov ss, word ptr [__w16_ss16]",
			"movzx esp, sp",
			"mov ax, 0x000a",
			"int 0x31",
			"setc cl",
			"mov ss, word ptr [__w16_ss32]",
			"movzx esp, sp",
			"pop ebx",
			"push ds",
			"pop es",
			in("dx") sel as u16,
			out("ax") new,
			out("cl") cf,
			options(nostack),
		);
	}
	if cf == 0 { new as u32 } else { 0 }
}

/// Frees a selector made by `dpmi_alias` (DPMI 01h).
fn dpmi_free(sel: u32) {
	unsafe {
		asm!(
			"push ebx",
			"mov bx, dx",
			"mov ss, word ptr [__w16_ss16]",
			"movzx esp, sp",
			"mov ax, 0x0001",
			"int 0x31",
			"mov ss, word ptr [__w16_ss32]",
			"movzx esp, sp",
			"pop ebx",
			"push ds",
			"pop es",
			in("dx") sel as u16,
			out("ax") _,
			options(nostack),
		);
	}
}

fn fill(buf: &mut [u8], xor: u8) {
	for (i, b) in buf.iter_mut().enumerate() {
		*b = (i as u8) ^ xor;
	}
}

/// Counts the bytes of `p[..n]` that differ from the pattern, reading through a volatile pointer so that nothing is cached.
fn bad_bytes(p: *const u8, n: usize, xor: u8) -> u32 {
	let p = core::hint::black_box(p);
	let mut bad = 0;
	for i in 0..n {
		if unsafe { read_volatile(p.add(i)) } != (i as u8) ^ xor {
			bad += 1;
		}
	}
	bad
}

/// Logs one step: the runtime's move count, the data segment's size and base, the alias's base, and the bytes that changed.
/// `stack` is the caller's pattern buffer (256 bytes) on the 32-bit stack.
fn verify(tag: &str, stack: *const u8) {
	let log = log();
	unsafe {
		STEPS += 1;
		let bad = bad_bytes(stack, 256, STACK_XOR)
			+ bad_bytes(addr_of!(DATA_PAT) as *const u8, 256, DATA_XOR);
		BYTES_BAD += bad;
		let ds = dpmi_base(data_selector());
		let (_ss16, ss32) = stack_selectors();
		let alias = dpmi_base(ss32);
		let stale = ds != alias;
		if stale {
			ALIAS_BAD += 1;
		}
		log.s("  ")
			.s(tag)
			.s(": moves ")
			.dec(stack_moves())
			.s(", ds base ")
			.hex(ds, 8)
			.s(", alias base ")
			.hex(alias, 8)
			.s(if stale { " STALE" } else { "" })
			.s(", bytes bad ")
			.dec(bad)
			.nl();
	}
	save_log();
}

/// Moveable global blocks, freed in an order that leaves holes, then `GlobalCompact`, so that Windows slides moveable segments.
fn step_global(stack: *const u8) {
	let mut h = [0u32; 40];
	let mut n = 0;
	while n < h.len() {
		let r = unsafe { GlobalAlloc(GMEM_MOVEABLE, 0x8000) };
		if r == 0 {
			break;
		}
		h[n] = r;
		n += 1;
	}
	let mut i = 1;
	while i < n {
		unsafe { GlobalFree(h[i]) };
		h[i] = 0;
		i += 2;
	}
	unsafe { GlobalCompact(0xFFFF_FFFF) };
	verify("global holes, compact", stack);
	let mut i = 1;
	while i < n {
		h[i] = unsafe { GlobalAlloc(GMEM_MOVEABLE, 0x2000) };
		i += 2;
	}
	unsafe { GlobalCompact(0xFFFF_FFFF) };
	verify("global refill, compact", stack);
	for x in h.iter().take(n) {
		if *x != 0 {
			unsafe { GlobalFree(*x) };
		}
	}
	unsafe { GlobalCompact(0xFFFF_FFFF) };
	verify("global freed, compact", stack);
}

/// Local heap growth: fixed blocks past the heap's initial size, one grown by `LocalReAlloc`, each holding a pattern that is checked
/// afterwards. Windows grows the local heap by growing the data segment, which is the path that moves it.
fn step_local(stack: *const u8) {
	let log = log();
	let mut blocks = [0u32; 120];
	let mut n = 0;
	let before = unsafe { GlobalSize(GlobalHandle(data_selector()) & 0xFFFF) };
	while n < blocks.len() {
		let p = unsafe { LocalAlloc(0, 256) };
		if p == 0 {
			break;
		}
		let b = p as *mut u8;
		for i in 0..64 {
			unsafe { *b.add(i) = (i as u8) ^ (n as u8) };
		}
		blocks[n] = p;
		n += 1;
	}
	let grown = unsafe { LocalReAlloc(blocks[0], 3000, 2) };
	if grown != 0 {
		blocks[0] = grown;
	}
	let mut bad = 0;
	for (k, blk) in blocks.iter().enumerate().take(n) {
		let p = core::hint::black_box(*blk as *const u8);
		for i in 0..64 {
			if unsafe { read_volatile(p.add(i)) } != (i as u8) ^ (k as u8) {
				bad += 1;
			}
		}
	}
	unsafe { BYTES_BAD += bad };
	let after = unsafe { GlobalSize(GlobalHandle(data_selector()) & 0xFFFF) };
	log.s("  local: ")
		.dec(n as u32)
		.s(" blocks of 256, ds size ")
		.dec(before)
		.s(" -> ")
		.dec(after)
		.s(", heap size ")
		.dec(unsafe { LocalHeapSize() })
		.s(", block bytes bad ")
		.dec(bad)
		.nl();
	verify("local grow", stack);
	unsafe { GlobalCompact(0xFFFF_FFFF) };
	verify("local grow, compact", stack);
	for k in 0..n {
		unsafe { LocalFree(blocks[k]) };
	}
	verify("local freed", stack);
}

/// Big moveable blocks, up to a few megabytes, freed in an odd order, then a compact.
fn step_big(stack: *const u8) {
	let mut h = [0u32; 16];
	let mut n = 0;
	while n < h.len() {
		let r = unsafe { GlobalAlloc(GMEM_MOVEABLE, 0x4_0000) };
		if r == 0 {
			break;
		}
		h[n] = r;
		n += 1;
	}
	log().s("  big: ").dec(n as u32).s(" blocks of 256 KB").nl();
	verify("big alloc", stack);
	let mut i = 0;
	while i < n {
		unsafe { GlobalFree(h[i]) };
		h[i] = 0;
		i += 3;
	}
	unsafe { GlobalCompact(0xFFFF_FFFF) };
	verify("big holes, compact", stack);
	let one = unsafe { GlobalAlloc(GMEM_MOVEABLE, 0x10_0000) };
	verify("big 1 MB", stack);
	if one != 0 {
		unsafe { GlobalFree(one) };
	}
	for x in h.iter().take(n) {
		if *x != 0 {
			unsafe { GlobalFree(*x) };
		}
	}
	unsafe { GlobalCompact(0xFFFF_FFFF) };
	verify("big freed, compact", stack);
}

/// `GlobalReAlloc` of the data segment's own handle, to a larger size that may not fit in place (the same thing Windows does to grow the
/// local heap), then a compact.
fn step_ds_realloc(stack: *const u8) {
	let log = log();
	let hds = unsafe { GlobalHandle(data_selector()) } & 0xFFFF;
	let size = unsafe { GlobalSize(hds) };
	let flags = unsafe { GlobalFlags(hds) };
	let r = unsafe { GlobalReAlloc(hds, size + 0x800, GMEM_MOVEABLE) };
	log.s("  ds realloc: handle ")
		.hex(hds, 4)
		.s(" size ")
		.dec(size)
		.s(" flags ")
		.hex(flags, 4)
		.s(" -> ")
		.hex(r, 4)
		.s(" size ")
		.dec(unsafe { GlobalSize(hds) })
		.nl();
	verify("ds realloc", stack);
	unsafe { GlobalCompact(0xFFFF_FFFF) };
	verify("ds realloc, compact", stack);
}

/// Everything the callback does to the data segment, with a pattern buffer on its own (32-bit) stack.
#[inline(never)]
fn force_moves(who: &str) {
	let log = log();
	let mut stack = [0u8; 256];
	fill(&mut stack, STACK_XOR);
	let p = core::hint::black_box(stack.as_ptr());
	log.s(who).s(": forcing, moves ").dec(stack_moves()).nl();
	verify("start", p);
	step_global(p);
	step_local(p);
	step_big(p);
	if unsafe { WITH_DS_REALLOC } {
		step_ds_realloc(p);
	}
	verify("done", p);
}

/// The timer callback of the message box variant.
extern "C" fn seg_timer(_hwnd: u32, msg: u32, _wparam: u32, _lparam: u32) -> u32 {
	if msg != WM_TIMER || unsafe { FORCED } {
		return 0;
	}
	unsafe {
		FORCED = true;
		CALLS += 1;
		MOVES_AT_CB = stack_moves();
		KillTimer(0, TIMER_ID);
	}
	force_moves("timer callback");
	unsafe {
		let mut hw = FindWindow(0, box_title());
		if hw == 0 {
			hw = GetActiveWindow();
			log()
				.s("  FindWindow found nothing; using the active window ")
				.hex(hw, 4)
				.nl();
		}
		DLG = hw;
		log().s("  closing dialog ").hex(hw, 4).nl();
		save_log();
		// A message box's own procedure ignores a posted IDOK without its button's handle; ending the dialog is the reliable close.
		EndDialog(hw, IDOK);
		PostMessage(hw, 0, 0, 0);
	}
	0
}

/// The hook of the `GetOpenFileName` variant: Windows calls it for every message the dialog gets.
extern "C" fn seg_hook(hwnd: u32, msg: u32, wparam: u32, _lparam: u32) -> u32 {
	unsafe {
		CALLS += 1;
		if msg == WM_INITDIALOG {
			DLG = hwnd;
			MOVES_AT_CB = stack_moves();
			log()
				.s("hook: WM_INITDIALOG, moves ")
				.dec(stack_moves())
				.nl();
			let mut stack = [0u8; 256];
			fill(&mut stack, STACK_XOR);
			let p = core::hint::black_box(stack.as_ptr());
			GlobalCompact(0xFFFF_FFFF);
			verify("hook init compact", p);
			SetTimer(hwnd, HOOK_TIMER, TIMER_MS, 0);
		} else if msg == WM_TIMER && wparam == HOOK_TIMER && !FORCED {
			FORCED = true;
			KillTimer(hwnd, HOOK_TIMER);
			force_moves("hook, WM_TIMER");
			log().s("  cancelling dialog ").hex(hwnd, 4).nl();
			save_log();
			let button = GetDlgItem(hwnd, IDCANCEL);
			PostMessage(hwnd, WM_COMMAND, IDCANCEL, button);
			return 1;
		}
	}
	0
}

static mut PATH: [u8; 144] = [0; 144];

fn save_log() {
	unsafe {
		let path = &mut *addr_of_mut!(PATH);
		let n = GetModuleFileName(HINST, path.as_mut_ptr() as u32, 130) as usize;
		let mut cut = n;
		while cut > 0 && path[cut - 1] != b'\\' && path[cut - 1] != b':' {
			cut -= 1;
		}
		for (i, b) in b"SEGMOVE.LOG\0".iter().enumerate() {
			path[cut + i] = *b;
		}
		write_file(path.as_ptr(), log().bytes());
	}
}

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

fn expect(name: &str, got: u32, want: u32) -> bool {
	let log = log();
	log.s(name).s(": ").dec(got);
	let ok = got == want;
	if !ok {
		log.s(" want ").dec(want).s(" FAIL");
	}
	log.nl();
	ok
}

/// Compares the control alias's base with the data segment's. Windows updates only its own selector, so the control alias keeps the
/// base the data segment had when it was made: the two differ when Windows moved the segment since, and agree when it did not. That is
/// a second witness to `stack_moves()` that does not depend on the runtime's bookkeeping. (Reading through a stale alias is not an
/// option: the old place is freed memory, and the read takes a page fault.) Returns false when the two witnesses disagree.
fn check_control(moves_since: u32) -> bool {
	let log = log();
	let ctl = unsafe { CONTROL };
	if ctl == 0 {
		log.s("control alias: DPMI refused, not checked").nl();
		return true;
	}
	let ds = dpmi_base(data_selector());
	let old = dpmi_base(ctl);
	let ok = (moves_since == 0) == (ds == old);
	log.s("control alias base ")
		.hex(old, 8)
		.s(", data segment base ")
		.hex(ds, 8)
		.s(", moves since it was made ")
		.dec(moves_since)
		.s(if ok { " (agree)" } else { " (DISAGREE)" })
		.nl();
	ok
}

/// The message box variant: a timer callback closes a `MessageBox`. `mine` lives on the program's own stack across the blocking call.
#[inline(never)]
fn variant_messagebox() -> bool {
	let log = log();
	let mut mine = [0u8; 256];
	fill(&mut mine, STACK_XOR);
	let p = core::hint::black_box(mine.as_ptr());
	let mut ok = true;
	unsafe {
		MOVES_AT_CALL = stack_moves();
		TIMER_ID = SetTimer(
			0,
			0,
			TIMER_MS,
			far(code_selector(), SegTimer as *const () as usize as u32),
		);
		log.s("variant MessageBox, timer ")
			.hex(TIMER_ID, 4)
			.s(", moves ")
			.dec(stack_moves())
			.nl();
		save_log();
		ok &= TIMER_ID != 0;
		let r = MessageBox(0, cs!("Segment move test"), box_title(), MB_OK);
		log.s("MessageBox returned ").dec(r).nl();
		ok &= expect("MessageBox result (IDOK)", r, IDOK);
	}
	verify("after MessageBox, main stack", p);
	ok
}

/// The common dialog variant: `GetOpenFileName` with a hook that forces the moves and cancels the dialog.
#[inline(never)]
fn variant_commdlg() -> bool {
	let log = log();
	let mut mine = [0u8; 256];
	fill(&mut mine, STACK_XOR);
	let p = core::hint::black_box(mine.as_ptr());
	let mut ok = true;
	unsafe {
		let o = addr_of_mut!(OFN);
		(*o).size = 72;
		(*o).filter = far(data_selector(), cs!("All files\0*.*\0\0"));
		(*o).filter_index = 1;
		(*o).file = far(data_selector(), addr_of_mut!(OFN_FILE) as u32);
		(*o).max_file = 260;
		(*o).initial_dir = far(data_selector(), cs!("C:\\"));
		(*o).title = far(data_selector(), cs!("Segment move test"));
		(*o).flags = OFN_ENABLEHOOK | OFN_HIDEREADONLY | OFN_NOCHANGEDIR;
		(*o).hook = far(code_selector(), SegHook as *const () as usize as u32);
		MOVES_AT_CALL = stack_moves();
		log.s("variant GetOpenFileName, moves ")
			.dec(stack_moves())
			.nl();
		save_log();
		let r = GetOpenFileName(o as u32);
		let ext = CommDlgExtendedError();
		log.s("GetOpenFileName returned ")
			.dec(r)
			.s(", extended error ")
			.hex(ext, 4)
			.nl();
		ok &= expect("GetOpenFileName result (cancelled)", r, 0);
		ok &= expect("CommDlgExtendedError", ext, 0);
	}
	verify("after GetOpenFileName, main stack", p);
	ok
}

#[unsafe(no_mangle)]
pub extern "C" fn w16_main(_hinst: u32, _hprev: u32, _show: u32) -> u32 {
	let log = log();
	unsafe {
		fill(&mut *addr_of_mut!(DATA_PAT), DATA_XOR);
		WITH_DS_REALLOC = command_has(b"/dsrealloc");
	}
	let commdlg = command_has(b"/cd");
	log.s("Rust Win16 segment move").nl();
	log.s("windows: ")
		.hex(unsafe { GetVersion() }, 8)
		.s(" flags ")
		.hex(unsafe { GetWinFlags() }, 8)
		.nl();
	let (ss16, ss32) = stack_selectors();
	log.s("stack ")
		.hex(ss16, 4)
		.s(", 32-bit stack ")
		.hex(ss32, 4);
	log.s(if ss16 == ss32 { " (no alias)" } else { "" }).nl();
	let hds = unsafe { GlobalHandle(data_selector()) } & 0xFFFF;
	log.s("data segment: handle ")
		.hex(hds, 4)
		.s(", flags ")
		.hex(unsafe { GlobalFlags(hds) }, 4)
		.s(", size ")
		.dec(unsafe { GlobalSize(hds) })
		.s(", local heap ")
		.dec(unsafe { LocalHeapSize() })
		.nl();

	unsafe {
		CONTROL = dpmi_alias(data_selector());
		MOVES_AT_CONTROL = stack_moves();
	}
	log.s("control alias ").hex(unsafe { CONTROL }, 4).nl();

	let mut ok = if commdlg {
		variant_commdlg()
	} else {
		variant_messagebox()
	};
	let end = stack_moves();
	let moved = end.wrapping_sub(unsafe { MOVES_AT_CALL });
	ok &= expect("callbacks run", unsafe { CALLS }.min(1), 1);
	log.s("steps checked: ").dec(unsafe { STEPS }).nl();
	ok &= unsafe { STEPS } > 8;
	ok &= check_control(end.wrapping_sub(unsafe { MOVES_AT_CONTROL }));
	ok &= expect("bytes changed", unsafe { BYTES_BAD }, 0);
	ok &= expect("alias stale", unsafe { ALIAS_BAD }, 0);
	log.s("callback entered at moves ")
		.dec(unsafe { MOVES_AT_CB })
		.s(", call began at ")
		.dec(unsafe { MOVES_AT_CALL })
		.nl();
	if unsafe { CONTROL } != 0 {
		dpmi_free(unsafe { CONTROL });
	}
	log.s(if ok { "RESULT: PASS" } else { "RESULT: FAIL" }).nl();
	log.s("MOVES: ").dec(moved).nl();
	let code = if ok { 0 } else { 1 };
	log.s("EXIT: ").dec(code).nl();
	save_log();
	unsafe {
		if command_has(b"/exit") {
			ExitWindows(0, 0);
		}
	}
	code
}
