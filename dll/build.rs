// Starts a library's link at `LibEntry`, which the loader calls, instead of the program entry point `link.ld` names.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

fn main() {
	println!("cargo:rustc-link-arg-bins=--entry=LibEntry");
}
