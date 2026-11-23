// The smallest Win16 program: one message box. Start here: it shows the whole path from source to an NE executable.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
#![no_std]
#![no_main]
use w16::*;

#[unsafe(no_mangle)]
pub extern "C" fn w16_main(_hinst: u32, _hprev: u32, _show: u32) -> u32 {
	unsafe { MessageBox(0, cs!("Hello from Rust"), cs!("Win16"), MB_OK) };
	0
}
