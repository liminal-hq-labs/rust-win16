// A wizard dialog driven by a three-page state machine: the text and the button label come from the string table.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
use crate::*;

/// The static text control and the default button every wizard template carries.
pub const ID_TEXT: u32 = 200;
pub const PAGES: u32 = 3;
/// String-table IDs: page text 1..=3, then the button label for each page, 4..=6.
const STR_TEXT: u32 = 1;
const STR_BUTTON: u32 = 4;

#[derive(PartialEq, Eq, Debug)]
pub enum Step {
	Show(u32),
	Finish,
	Cancel,
}

/// The state machine: what a click on button `id` does on page `state`.
pub fn step(state: u32, id: u32) -> Step {
	match id {
		IDCANCEL => Step::Cancel,
		IDOK if state + 1 >= PAGES => Step::Finish,
		IDOK => Step::Show(state + 1),
		_ => Step::Show(state),
	}
}

static mut STATE: u32 = 0;
static mut TEXT: [u8; 256] = [0; 256];

crate::export_proc!(WizDlgProc, wiz_dlgproc);

fn show(hdlg: u32, state: u32) {
	unsafe {
		let buf = core::ptr::addr_of_mut!(TEXT) as u32;
		LoadString(HINST, STR_TEXT + state, buf, 255);
		SetDlgItemText(hdlg, ID_TEXT, buf);
		LoadString(HINST, STR_BUTTON + state, buf, 255);
		SetDlgItemText(hdlg, IDOK, buf);
	}
}

extern "C" fn wiz_dlgproc(hdlg: u32, msg: u32, wparam: u32, _lparam: u32) -> u32 {
	unsafe {
		match msg {
			WM_INITDIALOG => {
				STATE = 0;
				show(hdlg, 0);
				1
			}
			WM_COMMAND => {
				match step(STATE, wparam) {
					Step::Show(s) => {
						STATE = s;
						show(hdlg, s);
					}
					Step::Finish => {
						EndDialog(hdlg, 1);
					}
					Step::Cancel => {
						EndDialog(hdlg, 0);
					}
				}
				1
			}
			_ => 0,
		}
	}
}

/// Runs the wizard dialog `dialog_id` modally. Returns 1 when the user finished, 0 when they cancelled.
pub fn run(parent: u32, dialog_id: u32) -> u32 {
	unsafe {
		DialogBox(
			HINST,
			dialog_id,
			parent,
			WizDlgProc as *const () as usize as u32,
		)
	}
}
