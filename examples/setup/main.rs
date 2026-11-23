// A Win16 SETUP.EXE in Rust: a welcome wizard, a copy dialog with progress, one bundled file copied to C:\W16TEST, an INI receipt and a
// result log, plus register checks before and after the dialogs.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
#![no_std]
#![no_main]
use core::ptr::addr_of_mut;
use w16::*;

export_proc!(ProgDlgProc, prog_dlgproc);

const DIR: *const u8 = b"C:\\W16TEST\0".as_ptr();
const FILE: *const u8 = b"C:\\W16TEST\\HELLO.TXT\0".as_ptr();
const INI: *const u8 = b"C:\\W16TEST\\INSTALL.INI\0".as_ptr();
const LOG_PATHS: [&[u8]; 3] = [
	b"C:\\W16TEST\\RESULT.LOG\0",
	b"D:\\RESULT.TXT\0",
	b"A:\\RESULT.TXT\0",
];
const CHUNK: u32 = 128;
/// Message-pump rounds after each chunk; they stand in for the time a slow floppy copy would take, so the progress dialog can be seen.
const PUMPS: u32 = 150;
const RT_RCDATA: u32 = 10;
const ID_STATUS: u32 = 200;
const ID_BAR: u32 = 201;

static mut CANCELLED: bool = false;
static mut MSG: Msg = Msg {
	hwnd: 0,
	message: 0,
	wparam: 0,
	lparam: 0,
	time: 0,
	pt: [0; 2],
};
static mut TEXT: [u8; 96] = [0; 96];
static mut BUF: [u8; 128] = [0; 128];

extern "C" fn prog_dlgproc(_hdlg: u32, msg: u32, wparam: u32, _lparam: u32) -> u32 {
	unsafe {
		match msg {
			WM_INITDIALOG => 1,
			WM_COMMAND => {
				if wparam == IDCANCEL {
					CANCELLED = true;
				}
				1
			}
			_ => 0,
		}
	}
}

/// Hands every queued message to Windows, the way a Win16 program lets other tasks run.
fn pump(hdlg: u32) {
	unsafe {
		let m = addr_of_mut!(MSG) as u32;
		while PeekMessage(m, 0, 0, 0, PM_REMOVE) != 0 {
			if hdlg == 0 || IsDialogMessage(hdlg, m) == 0 {
				TranslateMessage(m);
				DispatchMessage(m);
			}
		}
		Yield();
	}
}

fn set_progress(hdlg: u32, pct: u32) {
	unsafe {
		let t = &mut *addr_of_mut!(TEXT);
		let head = b"Copying HELLO.TXT: ";
		let mut n = 0;
		for &b in head {
			t[n] = b;
			n += 1;
		}
		if pct >= 100 {
			t[n] = b'1';
			n += 1;
		}
		if pct >= 10 {
			t[n] = b'0' + ((pct / 10) % 10) as u8;
			n += 1;
		}
		t[n] = b'0' + (pct % 10) as u8;
		n += 1;
		t[n] = b'%';
		t[n + 1] = 0;
		SetDlgItemText(hdlg, ID_STATUS, t.as_ptr() as u32);
		let bar = &mut t[48..];
		bar[0] = b'[';
		for i in 0..20u32 {
			bar[1 + i as usize] = if i * 5 < pct { b'#' } else { b'.' };
		}
		bar[21] = b']';
		bar[22] = 0;
		SetDlgItemText(hdlg, ID_BAR, bar.as_ptr() as u32);
	}
}

fn near_far(p: *const u8) -> u32 {
	far(data_selector(), p as u32)
}

/// Copies the RCDATA resource to C:\W16TEST\HELLO.TXT under a progress dialog. Returns (size, checksum) of what was copied.
fn copy_payload(log: &mut Log) -> Option<(u32, u32)> {
	unsafe {
		let hinst = HINST;
		let res = FindResource(hinst, int_resource(1), int_resource(RT_RCDATA));
		let _ = SizeofResource(hinst, res);
		if res == 0 {
			log.s("resource: FindResource failed").nl();
			return None;
		}
		let hmem = LoadResource(hinst, res);
		let locked = LockResource(hmem);
		// The packer puts the exact length in front of the data (resource sizes are rounded up to the alignment).
		let size = (far_byte(locked, 0) as u32)
			| ((far_byte(locked, 1) as u32) << 8)
			| ((far_byte(locked, 2) as u32) << 16)
			| ((far_byte(locked, 3) as u32) << 24);
		let src = locked + 4;
		log.s("resource: ")
			.dec(size)
			.s(" bytes at ")
			.hex(src, 8)
			.nl();
		if hmem == 0 || locked == 0 {
			return None;
		}

		let hdlg = CreateDialog(hinst, 101, 0, ProgDlgProc as *const () as usize as u32);
		if hdlg == 0 {
			log.s("progress dialog: CreateDialog failed").nl();
		}
		let mkdir = dos_mkdir(DIR);
		log.s("mkdir C:\\W16TEST: ").s(if mkdir == 0 {
			"created"
		} else {
			"not created, DOS error "
		});
		if mkdir != 0 {
			log.hex(mkdir as u32, 4);
		}
		log.nl();
		let h = _lcreat(FILE as u32, 0);
		if h == HFILE_ERROR {
			log.s("create HELLO.TXT: failed (handle ")
				.hex(h, 4)
				.s(")")
				.nl();
			return None;
		}
		let mut sum = 0u32;
		let mut off = 0u32;
		let mut ok = true;
		while off < size {
			let n = if size - off < CHUNK {
				size - off
			} else {
				CHUNK
			};
			if _lwrite_far(h, src + off, n) != n {
				ok = false;
				break;
			}
			for i in 0..n {
				sum = sum
					.wrapping_mul(31)
					.wrapping_add(far_byte(src, off + i) as u32);
			}
			off += n;
			set_progress(hdlg, off * 100 / size);
			for _ in 0..PUMPS {
				pump(hdlg);
			}
			if CANCELLED {
				ok = false;
				log.s("copy: cancelled by the user").nl();
				break;
			}
		}
		_lclose(h);
		FreeResource(hmem);
		if hdlg != 0 {
			DestroyWindow(hdlg);
		}
		if !ok {
			return None;
		}
		Some((size, sum))
	}
}

/// Reads the copied file back and returns its checksum.
fn verify_copy(log: &mut Log, size: u32) -> Option<u32> {
	unsafe {
		let h = _lopen(FILE as u32, 0);
		if h == HFILE_ERROR {
			log.s("verify: reopen failed").nl();
			return None;
		}
		let buf = addr_of_mut!(BUF) as *const u8;
		let mut sum = 0u32;
		let mut total = 0u32;
		loop {
			let n = _lread(h, near_far(buf), CHUNK);
			if n == 0 || n == 0xFFFF {
				break;
			}
			let b = &*addr_of_mut!(BUF);
			for i in 0..n as usize {
				sum = sum.wrapping_mul(31).wrapping_add(b[i] as u32);
			}
			total += n;
		}
		_lclose(h);
		log.s("verify: read back ")
			.dec(total)
			.s(" of ")
			.dec(size)
			.s(" bytes")
			.nl();
		if total == size { Some(sum) } else { None }
	}
}

fn write_receipt(log: &mut Log) -> bool {
	unsafe {
		let a =
			WritePrivateProfileString(cs!("RustWin16"), cs!("Product"), cs!("W16TEST"), INI as u32);
		let b = WritePrivateProfileString(cs!("RustWin16"), cs!("Files"), cs!("1"), INI as u32);
		let buf = addr_of_mut!(BUF) as u32;
		let n = GetPrivateProfileString(
			cs!("RustWin16"),
			cs!("Product"),
			cs!("none"),
			buf,
			64,
			INI as u32,
		);
		let all: &[u8] = &*addr_of_mut!(BUF);
		let got = &all[..n as usize];
		let ok = a != 0 && b != 0 && got == b"W16TEST";
		log.s("ini: wrote ")
			.dec(a + b)
			.s(" keys, read back ")
			.dec(n)
			.s(" bytes, ")
			.s(if ok { "match" } else { "MISMATCH" })
			.nl();
		ok
	}
}

fn describe_windows(log: &mut Log) {
	unsafe {
		let v = GetVersion();
		let flags = GetWinFlags();
		log.s("GetVersion=")
			.hex(v, 8)
			.s(" (Windows ")
			.dec(v & 0xFF)
			.s(".")
			.dec((v >> 8) & 0xFF)
			.s(")")
			.nl();
		log.s("GetWinFlags=").hex(flags, 8);
		if flags & 0x20 != 0 {
			log.s(" 386 Enhanced mode");
		}
		if flags & 0x10 != 0 {
			log.s(" Standard mode");
		}
		if flags & 0x02 != 0 {
			log.s(" CPU286");
		}
		if flags & 0x04 != 0 {
			log.s(" CPU386");
		}
		if flags & 0x08 != 0 {
			log.s(" CPU486");
		}
		log.nl();
	}
}

fn save_log(log: &mut Log) {
	for path in LOG_PATHS {
		let ok = write_file(path.as_ptr(), log.bytes());
		let _ = ok;
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn w16_main(_hinst: u32, _hprev: u32, _show: u32) -> u32 {
	let log = log();
	unsafe { SetErrorMode(1) }; // drives that are not ready fail quietly instead of asking
	log.s("Rust Win16 setup example (code16)").nl();
	describe_windows(log);
	let before = probe::run_all(log, "before the dialogs");

	let go = wizard::run(0, 100);
	if go != 1 {
		log.s("wizard: cancelled").nl().s("RESULT: CANCELLED").nl();
		save_log(log);
		return 0;
	}
	log.s("wizard: finished (3 pages)").nl();

	let mut pass = false;
	if let Some((size, sum)) = copy_payload(log)
		&& let Some(back) = verify_copy(log, size)
	{
		log.s("checksum: source ")
			.hex(sum, 8)
			.s(", copy ")
			.hex(back, 8)
			.nl();
		let ini = write_receipt(log);
		pass = back == sum && ini;
	}
	let after = probe::run_all(log, "after the dialogs");
	log.s("registers survived raw API calls: ")
		.s(if before && after { "yes" } else { "NO" })
		.nl();
	log.s(if pass { "RESULT: PASS" } else { "RESULT: FAIL" })
		.nl();
	save_log(log);
	unsafe {
		MessageBox(
			0,
			if pass {
				cs!("Setup complete. See C:\\W16TEST.")
			} else {
				cs!("Setup failed. See RESULT.LOG.")
			},
			cs!("Rust Win16 Setup"),
			if pass {
				MB_ICONINFORMATION
			} else {
				MB_ICONSTOP
			},
		);
	}
	if pass { 0 } else { 1 }
}
