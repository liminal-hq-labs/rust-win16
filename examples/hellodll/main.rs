// HELLODLL: the smallest library that shows the runtime: a version call, arithmetic, a write through a far pointer, a counter in the
// library's own data segment, a probe of its stacks and selectors, and a call that panics.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
//
// Every export takes one `u32` per Pascal word (see `w16dll::export_dll!`) and returns a `u32`. The ordinals are fixed in
// `exports.json` and the program that calls the library (`examples/dllhost`) repeats them.
#![no_std]
#![no_main]
use core::ptr::addr_of_mut;
use w16dll::*;

/// The ABI major version. A host accepts only the one it was built against; `HELLODLL_ABI_MAJOR` builds a library that differs.
const ABI_MAJOR: u32 = parse(option_env!("HELLODLL_ABI_MAJOR"), 1);
const ABI_MINOR: u32 = 0;
const BUILD_ID: u32 = 0x4C49_4D31;

const fn parse(text: Option<&str>, default: u32) -> u32 {
	match text {
		None => default,
		Some(text) => {
			let bytes = text.as_bytes();
			let mut value = 0;
			let mut i = 0;
			while i < bytes.len() {
				value = value * 10 + (bytes[i] - b'0') as u32;
				i += 1;
			}
			value
		}
	}
}

export_dll!(Info, info, 2);
export_dll!(Add, add, 2);
export_dll!(FillFar, fill_far, 4);
export_dll!(Poison, poison, 0);
export_dll!(Counter, counter, 0);
export_dll!(Probe, probe, 2);
export_dll!(EspWatch, esp_watch, 1);
export_dll!(Cross, cross, 2);

// DsSel(): the selector the loader patched into the thunks' `mov ax, <selector>`, with no runtime in the way: it returns the value the
// packer's fixup gave the library's data segment. It is a plain far procedure that takes nothing and keeps nothing.
core::arch::global_asm!(
	".code16",
	".section .text.exp_DsSel,\"ax\"",
	".globl DsSel",
	".globl __w16exp_DsSel",
	"DsSel:",
	"__w16exp_DsSel:",
	".byte 0xb8",
	".globl __w16sel_data_DsSel",
	"__w16sel_data_DsSel:",
	".word 0xffff",
	"xor dx, dx",
	".byte 0xcb",
	".section .data.w16keep.DsSel,\"aw\"",
	".byte 0",
	".reloc 0, R_386_NONE, DsSel",
);

static mut COUNTER: u32 = 0;

/// Info(far *InfoBlock): the size of the block (16), the ABI major and minor versions, the build id and the stack margin. The caller's
/// check that it has the library it was built for.
extern "C" fn info(sel: u32, off: u32) -> u32 {
	far_write16(sel, off, 16);
	far_write16(sel, off + 2, ABI_MAJOR);
	far_write16(sel, off + 4, ABI_MINOR);
	far_write32(sel, off + 6, BUILD_ID);
	far_write16(sel, off + 10, stack_margin());
	far_write16(sel, off + 12, instance());
	far_write16(sel, off + 14, 0);
	STATUS_OK
}

/// Add(a, b): the sum as a long, so that the high word of the result is checked too.
extern "C" fn add(a: u32, b: u32) -> u32 {
	a + b
}

/// FillFar(far *block, length, pattern): writes `pattern` to each of `length` bytes and returns the count.
extern "C" fn fill_far(sel: u32, off: u32, length: u32, pattern: u32) -> u32 {
	let mut i = 0;
	while i < length {
		far_write8(sel, off + i, pattern as u8);
		i += 1;
	}
	length
}

/// Poison(): panics. The caller gets `STATUS_INTERNAL`, and so does every later call, until the library is loaded again.
extern "C" fn poison() -> u32 {
	panic!("poison");
}

/// Counter(): adds one to a counter in the library's data segment and returns it. Every task that loads the library shares it.
extern "C" fn counter() -> u32 {
	unsafe {
		let counter = addr_of_mut!(COUNTER);
		*counter += 1;
		*counter
	}
}

/// Probe(far *words): twelve words that say where the call runs: the size (24), DS, the alias SS, the caller's SS and SP, the instance
/// handle, how often the alias has moved, the stack margin, the depth, the call count and the data segment's base (low, high).
extern "C" fn probe(sel: u32, off: u32) -> u32 {
	let (alias, ds) = selectors();
	let (caller_ss, caller_sp) = caller_stack();
	let base = base();
	let words = [
		24,
		ds,
		alias,
		caller_ss,
		caller_sp,
		instance(),
		moves(),
		stack_margin(),
		depth(),
		calls(),
		base & 0xFFFF,
		base >> 16,
	];
	let mut i = 0;
	while i < words.len() {
		far_write16(sel, off + 2 * i as u32, words[i]);
		i += 1;
	}
	STATUS_OK
}

/// EspWatch(rounds): how many of `rounds` samples of ESP had a non-zero high half on the private stack.
extern "C" fn esp_watch(rounds: u32) -> u32 {
	esp_dirty(rounds)
}

/// Cross(a, b): code in a second segment, reached by a direct call and by a call through a function pointer, both through the
/// packer's veneers on the private stack. Returns `a + b + 1000` plus `a * b` for even `b` and `a ^ b` for odd.
extern "C" fn cross(a: u32, b: u32) -> u32 {
	let pick: extern "C" fn(u32, u32) -> u32 = if b & 1 == 0 { cold_product } else { cold_xor };
	cold_sum(a, b) + pick(a, b)
}

#[unsafe(link_section = ".text.seg3.cold_sum")]
#[inline(never)]
extern "C" fn cold_sum(a: u32, b: u32) -> u32 {
	a + b + 1000
}

#[unsafe(link_section = ".text.seg3.cold_product")]
#[inline(never)]
extern "C" fn cold_product(a: u32, b: u32) -> u32 {
	a * b
}

#[unsafe(link_section = ".text.seg3.cold_xor")]
#[inline(never)]
extern "C" fn cold_xor(a: u32, b: u32) -> u32 {
	a ^ b
}
