// DLLHOST: loads HELLODLL.DLL the way a program that must keep running without it would, and checks the library on a real Windows.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// It loads the library itself (`SetErrorMode`, `LoadLibrary`, `GetProcAddress` by ordinal) and calls it through the dynamic stubs in
// `src/imports.rs`, so a missing library is a status, never a box from the loader. The log, DLLHOST.LOG beside the program, ends with
// `RESULT: PASS` or `RESULT: FAIL`.
//
// Command line (the first that applies; `/exit` may be added to any, and ends Windows when the run is done):
//   (none)       load HELLODLL.DLL by full path from the program's folder and run every check
//   /bare        load by its bare name, so the loader's search order finds it; the log says where it was found
//   /nomove      with the full run: do not fail when the library's data segment never moves (Wine never moves one)
//   /cwd         with /bare or /wrongver: first make C:\\ the current directory, so that the task's own directory is not it
//   /missing     expect the library to be absent: LoadLibrary must fail quietly
//   /wrongver    expect a library of another ABI major version: the check must refuse it
//   /share       load the library in two tasks: they share one data segment (the second task is `/peer`)
//   /hold        end Windows with the library still loaded (Windows calls WEP at system exit)
//   /count       count the free selectors, in a fresh Windows, to compare with the count at the end of a full run
//   /leak        load the library and end without freeing it, after starting `/watch`
//   /watch       see whether HELLODLL stays in memory once the task that loaded it has ended (log DLLWATCH.LOG)
#![no_std]
#![no_main]
use core::ptr::addr_of_mut;
use w16::*;

const GMEM_MOVEABLE: u32 = 2;
const GMEM_ZEROINIT: u32 = 0x40;
const SEM_FAILCRITICALERRORS: u32 = 1;
const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;
const ABI_MAJOR: u32 = 1;
const STATUS_INTERNAL: u32 = 7;

static mut PATH: [u8; 160] = [0; 160];
static mut LOG_PATH: [u8; 160] = [0; 160];
static mut INFO: [u16; 8] = [0; 8];
static mut PROBE: [u16; 12] = [0; 12];
static mut SELECTORS: [u16; 9000] = [0; 9000];

/// The PSP command tail: whether `flag` (lower case) appears in it.
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

/// Writes the program's folder followed by `name` (NUL-terminated) to `path` and returns its address, or the bare `name` when `bare`.
fn path_in(path: &mut [u8; 160], hinst: u32, name: &[u8], bare: bool) -> u32 {
	unsafe {
		let mut cut = 0;
		if !bare {
			let n = GetModuleFileName(hinst, path.as_mut_ptr() as u32, 130) as usize;
			cut = n;
			while cut > 0 && path[cut - 1] != b'\\' && path[cut - 1] != b':' {
				cut -= 1;
			}
		}
		for (i, b) in name.iter().enumerate() {
			path[cut + i] = *b;
		}
		path[cut + name.len()] = 0;
		path.as_ptr() as u32
	}
}

fn path_of(hinst: u32, name: &[u8], bare: bool) -> u32 {
	path_in(unsafe { &mut *addr_of_mut!(PATH) }, hinst, name, bare)
}

fn save_log(log: &Log, hinst: u32, name: &[u8]) {
	let path = path_in(unsafe { &mut *addr_of_mut!(LOG_PATH) }, hinst, name, false);
	write_file(path as *const u8, log.bytes());
}

static mut LOG_NAME: &[u8] = b"DLLHOST.LOG\0";

/// Logs one check, and writes the log out, so that a run that hangs shows how far it got.
fn check(log: &mut Log, name: &str, ok: bool) -> bool {
	log.s(name).s(": ").s(if ok { "ok" } else { "FAIL" }).nl();
	unsafe { save_log(log, HINST, *addr_of_mut!(LOG_NAME)) };
	ok
}

/// Loads the library and binds every export. Returns the module handle (under 32 is a failure code).
fn load(path: u32) -> u32 {
	unsafe {
		SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX);
		let h = LoadLibrary(path);
		if h >= 32 {
			bind_HelloInfo(GetProcAddress(h, int_resource(HelloInfo_ORDINAL)));
			bind_HelloAdd(GetProcAddress(h, int_resource(HelloAdd_ORDINAL)));
			bind_HelloFillFar(GetProcAddress(h, int_resource(HelloFillFar_ORDINAL)));
			bind_HelloPoison(GetProcAddress(h, int_resource(HelloPoison_ORDINAL)));
			bind_HelloCounter(GetProcAddress(h, int_resource(HelloCounter_ORDINAL)));
			bind_HelloProbe(GetProcAddress(h, int_resource(HelloProbe_ORDINAL)));
			bind_HelloEspWatch(GetProcAddress(h, int_resource(HelloEspWatch_ORDINAL)));
			bind_HelloCross(GetProcAddress(h, int_resource(HelloCross_ORDINAL)));
			bind_HelloDsSel(GetProcAddress(h, int_resource(HelloDsSel_ORDINAL)));
		}
		h
	}
}

/// Frees the library and unbinds the stubs, so a stale far pointer is never called.
fn unload(h: u32) {
	unsafe {
		FreeLibrary(h);
		bind_HelloInfo(0);
		bind_HelloAdd(0);
		bind_HelloFillFar(0);
		bind_HelloPoison(0);
		bind_HelloCounter(0);
		bind_HelloProbe(0);
		bind_HelloEspWatch(0);
		bind_HelloCross(0);
		bind_HelloDsSel(0);
	}
}

fn info() -> (u32, u32) {
	unsafe {
		let words = addr_of_mut!(INFO);
		let status = HelloInfo(words as u32);
		(status, (*words)[1] as u32)
	}
}

fn probe() -> [u32; 12] {
	unsafe {
		let words = addr_of_mut!(PROBE);
		HelloProbe(words as u32);
		let mut out = [0u32; 12];
		for (i, w) in out.iter_mut().enumerate() {
			*w = (*words)[i] as u32;
		}
		out
	}
}

/// How many selectors Windows will still hand out: allocates until it refuses, then frees them all. Nothing may be loaded afterwards:
/// Windows 3.11 hangs in the next `LoadLibrary`. Compare it with the same count taken by `/count` in a fresh Windows.
fn free_selectors() -> u32 {
	unsafe {
		let table = &mut *addr_of_mut!(SELECTORS);
		let mut n = 0;
		while n < table.len() {
			let sel = AllocSelector(0);
			if sel == 0 {
				break;
			}
			table[n] = sel as u16;
			n += 1;
		}
		for sel in table.iter().take(n) {
			FreeSelector(*sel as u32);
		}
		n as u32
	}
}

/// Moveable blocks freed in an order that leaves holes, then `GlobalCompact`: Windows slides moveable segments, the library's among them.
fn shake_memory() {
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
	let mut i = 1;
	while i < n {
		h[i] = unsafe { GlobalAlloc(GMEM_MOVEABLE, 0x2000) };
		i += 2;
	}
	unsafe { GlobalCompact(0xFFFF_FFFF) };
	for x in h.iter().take(n) {
		if *x != 0 {
			unsafe { GlobalFree(*x) };
		}
	}
	unsafe { GlobalCompact(0xFFFF_FFFF) };
}

fn selectors_of(p: &[u32; 12]) -> (u32, u32) {
	(p[2], p[1])
}

fn bare_name(name: &[u8]) -> u32 {
	path_of(0, name, true)
}

/// The module handle of HELLODLL, or 0 when it is not loaded.
fn dll_loaded() -> u32 {
	unsafe { GetModuleHandle(cs!("HELLODLL")) }
}

fn log_path_of(log: &mut Log, h: u32) {
	unsafe {
		let mut buf = [0u8; 144];
		let n = GetModuleFileName(h, buf.as_mut_ptr() as u32, 130) as usize;
		log.s("loaded from: ");
		for b in buf.iter().take(n) {
			let one = [*b];
			log.s(core::str::from_utf8_unchecked(&one));
		}
		log.nl();
	}
}

/// The task's current drive and directory, where the loader looks first for a library given by its bare name.
fn log_current_dir(log: &mut Log) {
	let mut dir = [0u8; 64];
	if let Some(drive) = dos_current_dir(dir.as_mut_ptr()) {
		log.s("current directory: ");
		let letter = [b'A' + drive as u8, b':', b'\\'];
		log.s(core::str::from_utf8(&letter).unwrap_or("?"));
		for b in dir.iter().take_while(|b| **b != 0) {
			let one = [*b];
			log.s(core::str::from_utf8(&one).unwrap_or("?"));
		}
		log.nl();
	}
}

/// With `/cwd`, moves the task's current directory to C:\\ (which holds no library), so that a bare name is not found there.
fn move_to_c_root(log: &mut Log) {
	if command_has(b"/cwd") {
		let error = dos_set_dir(cs!("C:\\\0") as *const u8, 2);
		log.s("moved to C:\\, DOS error ").dec(error as u32).nl();
		log_current_dir(log);
	}
}

fn run_checks(log: &mut Log, hinst: u32, bare: bool) -> bool {
	let mut ok = true;
	let path = if bare {
		bare_name(b"HELLODLL.DLL")
	} else {
		path_of(hinst, b"HELLODLL.DLL", false)
	};
	let mut h = load(path);
	log.s("LoadLibrary: ").hex(h, 4).nl();
	ok &= check(log, "loads", h >= 32);
	if h < 32 {
		return false;
	}
	log_path_of(log, h);
	let by_name = dll_loaded();
	log.s("GetModuleHandle(HELLODLL): ").hex(by_name, 4).nl();
	ok &= check(log, "found by module name", by_name != 0);
	unsafe {
		let upper = GetProcAddress(h, far(data_selector(), cs!("ADD")));
		let mixed = GetProcAddress(h, far(data_selector(), cs!("Add")));
		let missing = GetProcAddress(h, far(data_selector(), cs!("NoSuchExport")));
		log.s("GetProcAddress by name: ADD ")
			.hex(upper, 8)
			.s(", Add ")
			.hex(mixed, 8)
			.s(", NoSuchExport ")
			.hex(missing, 8)
			.nl();
		ok &= check(
			log,
			"export by name",
			upper != 0 && upper == GetProcAddress(h, int_resource(HelloAdd_ORDINAL)),
		);
		ok &= check(log, "missing export is refused", missing == 0);
	}

	let patched = unsafe { HelloDsSel() };
	log.s("selector the loader patched into the library: ")
		.hex(patched, 4)
		.nl();
	save_log(log, hinst, b"DLLHOST.LOG\0");
	let (status, major) = info();
	log.s("Info: status ")
		.dec(status)
		.s(", abi major ")
		.dec(major)
		.nl();
	ok &= check(log, "version", status == 0 && major == ABI_MAJOR);
	if major != ABI_MAJOR {
		return false;
	}

	// Arguments, results and a far pointer.
	let sum = unsafe { HelloAdd(40_000, 40_000) };
	log.s("Add(40000, 40000) = ").dec(sum).nl();
	ok &= check(log, "add", sum == 80_000);
	unsafe {
		let block = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 1024);
		let far_block = GlobalLock(block);
		let n = HelloFillFar(far_block, 1000, 0x5A);
		let mut bad = 0;
		for i in 0..1024 {
			let want = if i < 1000 { 0x5A } else { 0 };
			if far_byte(far_block, i) != want {
				bad += 1;
			}
		}
		log.s("FillFar returned ")
			.dec(n)
			.s(", wrong bytes ")
			.dec(bad)
			.nl();
		ok &= check(log, "far block", n == 1000 && bad == 0);
		GlobalUnlock(block);
		GlobalFree(block);
	}

	// The library's own data segment.
	let first = unsafe { HelloCounter() };
	let second = unsafe { HelloCounter() };
	ok &= check(
		log,
		"counter keeps state between calls",
		second == first + 1,
	);
	let p = probe();
	log.s("probe: ds ")
		.hex(p[1], 4)
		.s(" alias ")
		.hex(p[2], 4)
		.s(" caller ss ")
		.hex(p[3], 4)
		.s(" sp ")
		.hex(p[4], 4)
		.s(" hinst ")
		.hex(p[5], 4)
		.nl();
	log.s("       moves ")
		.dec(p[6])
		.s(" stack margin ")
		.dec(p[7])
		.s(" depth ")
		.dec(p[8])
		.s(" calls ")
		.dec(p[9])
		.s(" base ")
		.hex(p[11], 4)
		.hex(p[10], 4)
		.nl();
	let (host_ss, _) = stack_selectors();
	log.s("host ds ")
		.hex(data_selector(), 4)
		.s(" ss ")
		.hex(host_ss, 4)
		.nl();
	let patched = unsafe { HelloDsSel() };
	log.s("selector the loader patched into the library: ")
		.hex(patched, 4)
		.nl();
	ok &= check(
		log,
		"DS is the library's own",
		p[1] != data_selector() && p[1] != 0,
	);
	ok &= check(
		log,
		"the patched selector is the one the call ran with",
		patched == p[1],
	);
	ok &= check(
		log,
		"runs on the alias, not on DS or the caller's stack",
		p[2] != p[1] && p[2] != p[3] && p[2] != 0,
	);
	ok &= check(log, "the caller's stack is the program's", p[3] == host_ss);
	ok &= check(log, "one call at a time", p[8] == 1);
	ok &= check(log, "stack margin of 1024 bytes or more", p[7] >= 1024);
	let dirty = unsafe { HelloEspWatch(4_000_000) };
	log.s("esp watch on the private stack: ")
		.dec(dirty)
		.s(" dirty of 4000000")
		.nl();
	ok &= check(log, "esp high half is clean", dirty == 0);

	// Code in a second segment of the library, called directly and through a function pointer.
	let even = unsafe { HelloCross(2, 4) };
	let odd = unsafe { HelloCross(2, 3) };
	log.s("Cross(2, 4) = ")
		.dec(even)
		.s(", Cross(2, 3) = ")
		.dec(odd)
		.nl();
	ok &= check(
		log,
		"a second code segment, direct and through a pointer",
		even == 1014 && odd == 1006,
	);

	// Windows moves the library's data segment between calls.
	let before = probe();
	let mut moved = false;
	let mut rounds = 0;
	while rounds < 12 {
		shake_memory();
		let now = probe();
		if now[10] != before[10] || now[11] != before[11] {
			moved = true;
			break;
		}
		rounds += 1;
	}
	let after = probe();
	log.s("segment move: base ")
		.hex(before[11], 4)
		.hex(before[10], 4)
		.s(" -> ")
		.hex(after[11], 4)
		.hex(after[10], 4)
		.s(", alias moves ")
		.dec(before[6])
		.s(" -> ")
		.dec(after[6])
		.s(" after ")
		.dec(rounds + 1)
		.s(" rounds")
		.nl();
	// Wine never moves a segment, so `/nomove` accepts a library that stayed where it was.
	ok &= check(
		log,
		"data segment moved between calls",
		moved || command_has(b"/nomove"),
	);
	ok &= check(
		log,
		"alias followed the move",
		after[6] > before[6] || !moved,
	);
	let sum = unsafe { HelloAdd(1234, 4321) };
	ok &= check(log, "calls work after the move", sum == 5555);

	// A panic poisons the library until it is loaded again.
	let status = unsafe { HelloPoison() };
	log.s("Poison returned ").dec(status).nl();
	ok &= check(
		log,
		"panic returns the internal status",
		status == STATUS_INTERNAL,
	);
	let again = unsafe { HelloAdd(1, 2) };
	ok &= check(
		log,
		"poisoned library keeps refusing",
		again == STATUS_INTERNAL,
	);
	unload(h);
	ok &= check(log, "unloads after FreeLibrary", dll_loaded() == 0);
	h = load(path);
	ok &= check(log, "reloads", h >= 32);
	if h >= 32 {
		let sum = unsafe { HelloAdd(2, 3) };
		let c = unsafe { HelloCounter() };
		ok &= check(
			log,
			"a fresh load is not poisoned and starts over",
			sum == 5 && c == 1,
		);
		unload(h);
	}

	// Load and unload 200 times: no memory and no selectors lost.
	let space_before = unsafe { GetFreeSpace(0) };
	let mut cycle_ok = true;
	let mut alias_first = 0;
	let mut alias_changes = 0;
	for cycle in 0..200 {
		if cycle % 50 == 0 {
			log.s("cycle ")
				.dec(cycle)
				.s(", tick ")
				.dec(unsafe { GetTickCount() })
				.nl();
			unsafe { save_log(log, HINST, *addr_of_mut!(LOG_NAME)) };
		}
		let h = load(path);
		if h < 32 || unsafe { HelloAdd(7, 8) } != 15 {
			cycle_ok = false;
			if h >= 32 {
				unload(h);
			}
			break;
		}
		// WEP frees the alias, so the next load gets the same selector back; a leak would give a new one every time.
		let alias = probe()[2];
		if cycle == 0 {
			alias_first = alias;
		} else if alias != alias_first {
			alias_changes += 1;
		}
		unload(h);
	}
	let space_after = unsafe { GetFreeSpace(0) };
	log.s("200 cycles: GetFreeSpace ")
		.dec(space_before)
		.s(" -> ")
		.dec(space_after)
		.nl();
	ok &= check(log, "200 load and unload cycles", cycle_ok);
	log.s("alias selector in the first cycle ")
		.hex(alias_first, 4)
		.s(", cycles that got another ")
		.dec(alias_changes)
		.nl();
	ok &= check(
		log,
		"WEP frees the alias (the same selector comes back every cycle)",
		alias_changes == 0,
	);
	let lost = space_before.saturating_sub(space_after);
	ok &= check(log, "no memory lost (under 4 KB)", lost < 4096);
	ok &= check(log, "library is gone", dll_loaded() == 0);
	log.s("free selectors at the end: ")
		.dec(free_selectors())
		.nl();
	ok
}

#[unsafe(no_mangle)]
pub extern "C" fn w16_main(hinst: u32, _hprev: u32, _show: u32) -> u32 {
	let log = log();
	let mut ok = true;
	let mut name: &[u8] = b"DLLHOST.LOG\0";
	log.s("Rust Win16 DLL host").nl();
	log_current_dir(log);
	log.s("windows: ")
		.hex(unsafe { GetVersion() }, 8)
		.s(" flags ")
		.hex(unsafe { GetWinFlags() }, 8)
		.nl();
	if command_has(b"/missing") {
		log.s("mode: missing").nl();
		let h = load(path_of(hinst, b"HELLODLL.DLL", false));
		log.s("LoadLibrary: ").hex(h, 4).nl();
		ok &= check(log, "a missing library fails quietly", h < 32);
		if h >= 32 {
			unload(h);
		}
		let status = unsafe { HelloAdd(1, 2) };
		ok &= check(log, "an unbound stub reports missing", status == 0xFFFF);
	} else if command_has(b"/peer") {
		// The second task of `/share`: one library, loaded by two tasks, has one data segment, so the counter goes on from the first.
		log.s("mode: peer").nl();
		name = b"DLLPEER.LOG\0";
		unsafe { LOG_NAME = name };
		let h = load(path_of(hinst, b"HELLODLL.DLL", false));
		ok &= check(log, "loads", h >= 32);
		let counted = unsafe { HelloCounter() };
		log.s("counter in the second task: ").dec(counted).nl();
		ok &= check(log, "the data segment is the first task's", counted == 2);
		let p = probe();
		log.s("probe: ds ")
			.hex(p[1], 4)
			.s(" alias ")
			.hex(p[2], 4)
			.s(" caller ss ")
			.hex(p[3], 4)
			.s(" hinst ")
			.hex(p[5], 4)
			.nl();
		if h >= 32 {
			unload(h);
		}
	} else if command_has(b"/share") {
		log.s("mode: share").nl();
		let h = load(path_of(hinst, b"HELLODLL.DLL", false));
		ok &= check(log, "loads", h >= 32);
		let first = unsafe { HelloCounter() };
		let (alias, ds) = selectors_of(&probe());
		let me = path_of(hinst, b"DLLHOST.EXE /peer", false);
		let started = unsafe { WinExec(me, SW_SHOW) };
		ok &= check(log, "second task started", started >= 32);
		let start = unsafe { GetTickCount() };
		let mut seen_two = false;
		while unsafe { GetTickCount() }.wrapping_sub(start) < 20_000 {
			// Looked at before each yield: the second task may run to its end inside the first one.
			let usage = unsafe { GetModuleUsage(hinst) };
			if usage >= 2 {
				seen_two = true;
			}
			if usage == 1 {
				break;
			}
			unsafe { Yield() };
		}
		// WinExec returns once the new task has run to its first message wait or its end, so the count may never be seen at 2.
		log.s("second task seen running: ")
			.dec(seen_two as u32)
			.nl();
		ok &= check(
			log,
			"second task is gone",
			unsafe { GetModuleUsage(hinst) } == 1,
		);
		let last = unsafe { HelloCounter() };
		log.s("counter: first task ")
			.dec(first)
			.s(", after the second task ")
			.dec(last)
			.nl();
		ok &= check(
			log,
			"one data segment for both tasks",
			first == 1 && last == 3,
		);
		let (alias2, ds2) = selectors_of(&probe());
		ok &= check(
			log,
			"the library's selectors are the same for both",
			alias == alias2 && ds == ds2,
		);
		ok &= check(
			log,
			"still loaded for the first task after the second freed it",
			dll_loaded() != 0,
		);
		unload(h);
		ok &= check(log, "unloaded with the last user", dll_loaded() == 0);
	} else if command_has(b"/hold") {
		// Ends Windows with the library still loaded, so that Windows calls `WEP` at system exit. The run script reports whether Windows ended.
		log.s("mode: hold").nl();
		let h = load(path_of(hinst, b"HELLODLL.DLL", false));
		ok &= check(log, "loads", h >= 32);
		ok &= check(log, "add", unsafe { HelloAdd(20, 22) } == 42);
		log.s("ending Windows with the library loaded").nl();
	} else if command_has(b"/count") {
		log.s("mode: count").nl();
		log.s("free selectors at the start: ")
			.dec(free_selectors())
			.nl();
	} else if command_has(b"/wrongver") {
		let bare = command_has(b"/bare");
		log.s("mode: wrong version").nl();
		move_to_c_root(log);
		let h = load(path_of(hinst, b"HELLODLL.DLL", bare));
		ok &= check(log, "loads", h >= 32);
		if h >= 32 {
			log_path_of(log, h);
			let (_, major) = info();
			log.s("abi major ")
				.dec(major)
				.s(", wanted ")
				.dec(ABI_MAJOR)
				.nl();
			ok &= check(log, "version mismatch is seen", major != ABI_MAJOR);
			unload(h);
		}
	} else if command_has(b"/leak") {
		log.s("mode: leak").nl();
		name = b"DLLLEAK.LOG\0";
		unsafe { LOG_NAME = name };
		let h = load(path_of(hinst, b"HELLODLL.DLL", false));
		ok &= check(log, "loads", h >= 32);
		let me = path_of(hinst, b"DLLHOST.EXE /watch /exit", false);
		let started = unsafe { WinExec(me, SW_SHOW) };
		log.s("WinExec: ").dec(started).nl();
		ok &= check(log, "watcher started", started >= 32);
		// Wait for the watcher to be running, give it time to look, then end without FreeLibrary.
		let mut i = 0;
		while i < 20_000 && unsafe { GetModuleUsage(hinst) } < 2 {
			unsafe { Yield() };
			i += 1;
		}
		let mut j = 0;
		while j < 3000 {
			unsafe { Yield() };
			j += 1;
		}
		log.s("leaving without FreeLibrary; library still loaded: ")
			.dec((dll_loaded() != 0) as u32)
			.nl();
	} else if command_has(b"/watch") {
		log.s("mode: watch").nl();
		name = b"DLLWATCH.LOG\0";
		unsafe { LOG_NAME = name };
		let first = dll_loaded();
		log.s("first look: HELLODLL handle ")
			.hex(first, 4)
			.s(", uses ")
			.dec(unsafe { GetModuleUsage(first) })
			.s(", DLLHOST instances ")
			.dec(unsafe { GetModuleUsage(hinst) })
			.nl();
		ok &= check(
			log,
			"library is loaded while the other task runs",
			first != 0,
		);
		// Wait until the task that loaded the library has gone, then give Windows a few seconds to unload it, if it ever does.
		let start = unsafe { GetTickCount() };
		let mut gone = false;
		let mut gone_at = 0;
		while unsafe { GetTickCount() }.wrapping_sub(start) < 12_000 {
			unsafe { Yield() };
			if gone_at == 0 && unsafe { GetModuleUsage(hinst) } == 1 {
				gone_at = unsafe { GetTickCount() }.wrapping_sub(start).max(1);
			}
			if dll_loaded() == 0 {
				gone = true;
				break;
			}
			if gone_at != 0 && unsafe { GetTickCount() }.wrapping_sub(start) > gone_at + 4000 {
				break;
			}
		}
		log.s("the loading task ended after ")
			.dec(gone_at)
			.s(" ms; DLLHOST instances now ")
			.dec(unsafe { GetModuleUsage(hinst) })
			.nl();
		log.s("OBSERVED: after its task ended without FreeLibrary the library is ")
			.s(if gone { "unloaded" } else { "still loaded" })
			.nl();
		ok &= check(
			log,
			"the loading task ended, so the observation is valid",
			gone_at != 0,
		);
	} else {
		let bare = command_has(b"/bare");
		log.s(if bare {
			"mode: bare name"
		} else {
			"mode: full path"
		})
		.nl();
		move_to_c_root(log);
		ok &= run_checks(log, hinst, bare);
	}
	log.s(if ok { "RESULT: PASS" } else { "RESULT: FAIL" }).nl();
	save_log(log, hinst, name);
	unsafe {
		if command_has(b"/exit") && !command_has(b"/leak") {
			ExitWindows(0, 0);
		}
	}
	if ok { 0 } else { 1 }
}
