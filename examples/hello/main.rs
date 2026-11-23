// Hello Win16: a message box, then a window with a message loop that opens a native wizard dialog.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
#![no_std]
#![no_main]
use core::ptr::addr_of_mut;
use w16::*;

export_proc!(HelloWndProc, hello_wndproc);

static mut MSG: Msg = Msg {
	hwnd: 0,
	message: 0,
	wparam: 0,
	lparam: 0,
	time: 0,
	pt: [0; 2],
};
static mut PS: PaintStruct = PaintStruct {
	hdc: 0,
	erase: 0,
	rect: [0; 4],
	restore: 0,
	inc_update: 0,
	reserved: [0; 16],
};
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

extern "C" fn hello_wndproc(hwnd: u32, msg: u32, wparam: u32, lparam: u32) -> u32 {
	unsafe {
		match msg {
			WM_CREATE => {
				PostMessage(hwnd, WM_USER, 0, 0);
				0
			}
			WM_USER => {
				wizard::run(hwnd, 100);
				DestroyWindow(hwnd);
				0
			}
			WM_PAINT => {
				let ps = addr_of_mut!(PS);
				let hdc = BeginPaint(hwnd, ps as u32);
				let text = b"Rust, 16-bit code, one NE executable";
				TextOut(hdc, 8, 8, text.as_ptr() as u32, text.len() as u32);
				EndPaint(hwnd, ps as u32);
				0
			}
			WM_DESTROY => {
				PostQuitMessage(0);
				0
			}
			_ => DefWindowProc(hwnd, msg, wparam, lparam),
		}
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn w16_main(hinst: u32, hprev: u32, show: u32) -> u32 {
	unsafe {
		MessageBox(
			0,
			cs!("Hello from Rust, in 16-bit Windows"),
			cs!("Rust Win16"),
			MB_ICONINFORMATION,
		);
		if hprev == 0 {
			let c = addr_of_mut!(CLASS);
			(*c).wndproc = far(code_selector(), HelloWndProc as *const () as usize as u32);
			(*c).instance = hinst as u16;
			(*c).cursor = LoadCursor(0, int_resource(32512)) as u16; // IDC_ARROW
			(*c).background = 6; // COLOR_WINDOW + 1
			(*c).class_name = far(data_selector(), cs!("LimHello"));
			if RegisterClass(c as u32) == 0 {
				return 1;
			}
		}
		let hwnd = CreateWindow(
			cs!("LimHello"),
			cs!("Rust Win16 hello"),
			0x00CF_0000,
			0x8000,
			0x8000,
			0x8000,
			0x8000,
			0,
			0,
			hinst,
			0,
		);
		if hwnd == 0 {
			return 2;
		}
		ShowWindow(hwnd, show);
		UpdateWindow(hwnd);
		let m = addr_of_mut!(MSG) as u32;
		while GetMessage(m, 0, 0, 0) != 0 {
			TranslateMessage(m);
			DispatchMessage(m);
		}
		(*addr_of_mut!(MSG)).wparam as u32
	}
}
