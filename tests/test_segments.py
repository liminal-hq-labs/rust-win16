#!/usr/bin/env python3
# Tests for multi-segment packing: placement, the segment table, veneers and their fixups, function-pointer slots, the gate for
# load-on-call segments, the shadow stack and the cases the packer refuses. tools/ne_dump.py reads the output; tools/ne_check.py
# decodes it when objdump is installed.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# usage: python3 -m unittest discover -s tests -v
import os
import shutil
import struct
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tools"))

import gen_imports  # noqa: E402
import ne_addr2sym  # noqa: E402
import ne_check  # noqa: E402
import ne_dump  # noqa: E402
import ne_pack  # noqa: E402
from rust_demangle import demangle  # noqa: E402

CODE_BASE = 0x100000
RET = b"\x66\xc3"


class Elf:
	"""Builds a linked ELF the way link.ld leaves one: `.data` at 0, each code section its own output section above 0x100000, symbols
	and `.rel.*` sections with the resolved values in place."""

	def __init__(self, data=bytes(16)):
		self.code = []  # [name, bytearray, addr, align]
		self.data = bytearray(data)
		self.syms = []  # (name, section name, address, type)
		self.rels = []  # (section name, address, type, symbol name)
		self.next = CODE_BASE

	def section(self, name, body, align=1, func=None):
		self.next = (self.next + align - 1) & ~(align - 1)
		addr = self.next
		self.code.append([name, bytearray(body), addr, align])
		self.next += len(body)
		self.syms.append((func or name, name, addr, 2))
		return addr

	def addr(self, name):
		return next(c[2] for c in self.code if c[0] == name)

	def symbol(self, name, section, addr, typ=0):
		self.syms.append((name, section, addr, typ))

	def _body(self, section):
		if section == ".data":
			return self.data, 0
		c = next(c for c in self.code if c[0] == section)
		return c[1], c[2]

	def _value(self, name):
		return next(s[2] for s in self.syms if s[0] == name)

	def call(self, section, offset, target, opcode=b"\x66\xe8"):
		"""Writes a 32-bit relative call (or jump) at `offset` in `section` to symbol `target`, with its relocation."""
		body, base = self._body(section)
		body[offset : offset + len(opcode)] = opcode
		p = base + offset + len(opcode)
		struct.pack_into("<i", body, offset + len(opcode), self._value(target) - (p + 4))
		self.rels.append((section, p, ne_pack.R_386_PC32, target))

	def jump16(self, section, offset, target):
		body, base = self._body(section)
		body[offset] = 0xE9
		p = base + offset + 1
		struct.pack_into("<H", body, offset + 1, (self._value(target) - (p + 2)) & 0xFFFF)
		self.rels.append((section, p, ne_pack.R_386_PC16, target))

	def pointer(self, section, offset, target, addend=0):
		body, base = self._body(section)
		struct.pack_into("<I", body, offset, (self._value(target) + addend) & 0xFFFFFFFF)
		self.rels.append((section, base + offset, ne_pack.R_386_32, target))

	def build(self):
		names = [".data"] + [c[0] for c in self.code]
		rel_secs = sorted({r[0] for r in self.rels}, key=names.index)
		shnames = names + [".rel" + n for n in rel_secs] + [".symtab", ".strtab", ".shstrtab"]
		shstr = b"\0"
		name_off = {}
		for n in shnames:
			name_off[n] = len(shstr)
			shstr += n.encode() + b"\0"
		index = {n: i + 1 for i, n in enumerate(shnames)}
		strtab = b"\0"
		symtab = bytes(16)
		sym_index = {}
		for name, sec, addr, typ in self.syms:
			sym_index.setdefault(name, len(symtab) // 16)
			shndx = 0 if sec is None else index[sec]
			symtab += struct.pack("<IIIBBH", len(strtab), addr, 0, 0x10 | typ, 0, shndx)
			strtab += name.encode() + b"\0"
		for name in {r[3] for r in self.rels} - set(sym_index):
			sym_index[name] = len(symtab) // 16
			symtab += struct.pack("<IIIBBH", len(strtab), 0, 0, 0x10, 0, 0)
			strtab += name.encode() + b"\0"
		blobs = [bytes(self.data)] + [bytes(c[1]) for c in self.code]
		rel_blobs = []
		for rs in rel_secs:
			b = b""
			for sec, addr, typ, sym in self.rels:
				if sec == rs:
					b += struct.pack("<II", addr, (sym_index[sym] << 8) | typ)
			rel_blobs.append(b)
		off = 52
		body = b""
		heads = [(0,) * 10]
		symtab_i = index[".symtab"]
		for i, n in enumerate(names):
			blob = blobs[i]
			if n == ".data":
				heads.append((name_off[n], 1, 3, 0, off + len(body), len(blob), 0, 0, 4, 0))
			else:
				c = self.code[i - 1]
				heads.append((name_off[n], 1, 6, c[2], off + len(body), len(blob), 0, 0, c[3], 0))
			body += blob
		for rs, b in zip(rel_secs, rel_blobs):
			heads.append((name_off[".rel" + rs], 9, 0x40, 0, off + len(body), len(b), symtab_i, index[rs], 4, 8))
			body += b
		heads.append((name_off[".symtab"], 2, 0, 0, off + len(body), len(symtab), symtab_i + 1, 1, 4, 16))
		body += symtab
		heads.append((name_off[".strtab"], 3, 0, 0, off + len(body), len(strtab), 0, 0, 1, 0))
		body += strtab
		heads.append((name_off[".shstrtab"], 3, 0, 0, off + len(body), len(shstr), 0, 0, 1, 0))
		body += shstr
		sh_off = off + len(body)
		hdr = b"\x7fELF" + bytes([1, 1, 1, 0]) + bytes(8)
		hdr += struct.pack("<HHIIIIIHHHHHH", 2, 3, 1, 0, 0, sh_off, 0, 52, 0, 0, 40, len(heads), len(heads) - 1)
		return hdr + body + b"".join(struct.pack("<IIIIIIIIII", *h) for h in heads)


def two_segments():
	"""`.text.start` (segment 1) calls `far_fn`, which an attribute-style section name puts in segment 3; `far_fn` calls back."""
	e = Elf()
	e.section(".text.start", b"\x90" * 2 + bytes(6) + RET + b"\x90" * 4, func="_start")
	e.section(".text.seg3.far_fn", bytes(6) + RET, func="far_fn")
	e.section(".text.near_fn", b"\x90" + RET, func="near_fn")
	e.call(".text.start", 2, "far_fn")
	e.call(".text.seg3.far_fn", 0, "near_fn")
	return e


def with_gate_symbols(e):
	"""Adds what the gate needs from the runtime: the two stack selectors in the data and the alias sync routine in segment 1."""
	e.data += bytes(4)
	e.symbol("__w16_ss16", ".data", 16)
	e.symbol("__w16_ss32", ".data", 18)
	e.section(".text.w16fixed.sync", RET, func="w16_sync_alias")
	return e


def segments(out):
	return {s["index"]: s for s in ne_dump.parse(out)["segments"]}


def code_of(out, seg):
	return out[seg["file_off"] : seg["file_off"] + seg["length"]]


class SegmentTests(unittest.TestCase):
	def test_relocations_are_recomputed_in_one_segment(self):
		e = Elf()
		e.section(".text.start", bytes(6) + RET, func="_start")
		e.section(".text.b", b"\x90" * 3 + RET, align=4, func="b")
		e.call(".text.start", 0, "b")
		out, info = ne_pack.pack(e.build(), module="T")
		segs = segments(out)
		self.assertEqual(sorted(segs), [1, 2])
		code = code_of(out, segs[1])
		self.assertEqual(code[:2], b"\x66\xe8")
		self.assertEqual(struct.unpack_from("<i", code, 2)[0], 8 - 6)  # b is aligned to 4, at offset 8; the call ends at 6
		self.assertEqual(info["segments"][0]["number"], 1)

	def test_segment_table_order_and_flags(self):
		out, info = ne_pack.pack(two_segments().build(), module="T")
		h = ne_dump.parse(out)["header"]
		self.assertEqual((h["nseg"], h["autodata"], h["ss"]), (3, 2, 2))
		segs = segments(out)
		self.assertEqual(segs[1]["flags"], 0x0D40)  # fixed, preloaded, relocations
		self.assertEqual(segs[2]["flags"], 0x0C41)  # the automatic data segment
		self.assertEqual(segs[3]["flags"], 0x0D50)  # moveable, preloaded, relocations
		self.assertEqual([s["number"] for s in info["segments"]], [1, 3])
		for s in segs.values():
			self.assertEqual(s["file_off"] % 16, 0)

	def test_a_cross_segment_call_goes_through_a_veneer(self):
		out, info = ne_pack.pack(two_segments().build(), module="T")
		segs = segments(out)
		code1, code3 = code_of(out, segs[1]), code_of(out, segs[3])
		start = next(o for s, o, _z, n in info["map"] if s == 1 and n == "start")
		self.assertEqual(code1[start + 2 : start + 4], b"\x66\xe9")  # the call became a jump to the veneer
		site_end = start + 8
		veneer = site_end + struct.unpack_from("<i", code1, start + 4)[0]
		self.assertEqual(code1[veneer : veneer + 2], b"\x66\x68")  # push dword: the return trampoline
		tramp = struct.unpack_from("<I", code1, veneer + 2)[0]
		self.assertEqual(code1[veneer + 6], 0xEA)  # jmp far
		far_fn = next(o for s, o, _z, n in info["map"] if s == 3 and n == "seg3.far_fn")
		r1 = {r["offset"]: r for r in segs[1]["relocs"]}
		self.assertEqual(r1[veneer + 7]["target"], {"seg": 3, "off": far_fn})
		self.assertEqual(struct.unpack_from("<H", code1, veneer + 7)[0], 0xFFFF)  # a chain of one
		self.assertEqual(code3[tramp], 0xEA)  # the return trampoline in segment 3 jumps back after the call
		r3 = {r["offset"]: r for r in segs[3]["relocs"]}
		self.assertEqual(r3[tramp + 1]["target"], {"seg": 1, "off": site_end})
		self.assertEqual(info["veneers"], 2)  # and far_fn's call back to near_fn

	def test_entry_point_and_exports_stay_in_segment_1(self):
		e = two_segments()
		e.section(".text.exp_WndProc", RET, func="WndProc")
		e.symbol("__w16exp_WndProc", ".text.exp_WndProc", e.addr(".text.exp_WndProc"))
		out, _i = ne_pack.pack(e.build(), module="T")
		info = ne_dump.parse(out)
		self.assertEqual(info["header"]["cs"], 1)
		self.assertEqual([(x["seg"], x["flags"]) for x in info["entries"]], [(1, 3)])

	def test_an_export_elsewhere_is_refused(self):
		e = two_segments()
		e.symbol("__w16exp_Bad", ".text.seg3.far_fn", e.addr(".text.seg3.far_fn"))
		with self.assertRaisesRegex(ne_pack.PackError, "segment 1"):
			ne_pack.pack(e.build(), module="T")

	def test_function_pointers_use_slots_in_every_segment(self):
		e = two_segments()
		e.data += bytes(8)
		e.pointer(".data", 16, "far_fn")
		e.pointer(".data", 20, "near_fn")
		out, info = ne_pack.pack(e.build(), module="T")
		segs = segments(out)
		slots = dict(info["slots"])
		self.assertEqual(sorted(slots), ["near_fn", "seg3.far_fn"])
		self.assertEqual(slots["seg3.far_fn"], slots["near_fn"] + ne_pack.SLOT_SIZE)  # slots sort by segment, so far_fn's is second
		d = code_of(out, segs[2])
		self.assertEqual(struct.unpack_from("<I", d, 16)[0], slots["seg3.far_fn"])
		self.assertEqual(struct.unpack_from("<I", d, 20)[0], slots["near_fn"])
		code1, code3 = code_of(out, segs[1]), code_of(out, segs[3])
		at = slots["seg3.far_fn"]
		self.assertEqual(code3[at], 0xE9)  # in its own segment, a near jump
		self.assertEqual(code1[at : at + 1] + code1[at + 3 : at + 4], b"\x68\x68")  # elsewhere: push selector, push offset
		self.assertEqual(code1[at + 6], 0xE9)  # and the dynamic veneer
		r1 = {r["offset"]: r for r in segs[1]["relocs"]}
		self.assertEqual((r1[at + 1]["src"], r1[at + 1]["target"]["seg"]), ("selector16", 3))
		self.assertGreater(at, 4)  # never offset 0, which a null pointer would call

	def test_tail_jumps_across_segments_use_a_stub(self):
		e = Elf()
		e.section(".text.start", bytes(6) + RET, func="_start")
		e.section(".text.hop", bytes(3), func="hop")
		e.section(".text.seg3.target", RET, func="target")
		e.call(".text.start", 0, "hop")
		e.jump16(".text.hop", 0, "target")
		out, info = ne_pack.pack(e.build(), module="T")
		self.assertEqual(info["stubs"], 1)
		code1 = code_of(out, segments(out)[1])
		hop = next(o for s, o, _z, n in info["map"] if n == "hop")
		stub = (hop + 3 + struct.unpack_from("<h", code1, hop + 1)[0]) & 0xFFFF
		self.assertEqual(code1[stub], 0x68)
		self.assertEqual(code1[stub + 6], 0xE9)

	def test_shadow_stack_follows_the_data(self):
		out, info = ne_pack.pack(two_segments().build(), module="T", shadow=64)
		ssp, depth = info["shadow"]
		self.assertEqual(depth, 64)
		d = code_of(out, segments(out)[2])
		self.assertEqual(len(d), ssp + 4 + 4 * 64)
		self.assertEqual(struct.unpack_from("<H", d, ssp)[0], ssp + 4)  # the shadow stack pointer starts at the base
		with self.assertRaises(ne_pack.PackError):
			ne_pack.pack(two_segments().build(), module="T", shadow=8)

	def test_a_full_shadow_stack_jumps_to_the_runtime_handler(self):
		e = two_segments()
		e.section(".text.w16fixed.shadow_overflow", RET, func="w16_shadow_overflow")
		out, _i = ne_pack.pack(e.build(), module="T")
		segs = segments(out)
		handler = [r for r in segs[3]["relocs"] if r.get("target", {}).get("seg") == 1 and r["src"] == "far"]
		self.assertTrue(handler)

	def test_config_places_by_rust_path_and_sets_flags(self):
		e = Elf()
		e.section(".text.start", bytes(6) + RET, func="_start")
		e.section(".text._RNvNtCs1234_4mycr4cold4slow", RET, func="_RNvNtCs1234_4mycr4cold4slow")
		e.call(".text.start", 0, "_RNvNtCs1234_4mycr4cold4slow")
		with_gate_symbols(e)
		cfg = {"segments": {"3": {"place": ["mycr::cold::*"], "preload": False, "discardable": True}}}
		out, info = ne_pack.pack(e.build(), module="T", segments=cfg)
		self.assertEqual(segments(out)[3]["flags"], 0x1D10)  # discardable, moveable, relocations; not preloaded
		self.assertEqual([m[3] for m in info["map"] if m[0] == 3 and not m[3].startswith("(")], ["mycr::cold::slow"])

	def test_load_on_call_segments_need_the_gate(self):
		e = two_segments()
		cfg = {"segments": {"3": {"place": ["seg3.*"], "preload": False}}}
		with self.assertRaisesRegex(ne_pack.PackError, "gate"):
			ne_pack.pack(e.build(), module="T", segments=cfg)
		with_gate_symbols(e)
		out, info = ne_pack.pack(e.build(), module="T", segments=cfg)
		segs = segments(out)
		code1 = code_of(out, segs[1])
		gate = code1.find(bytes([0x66, 0x60, 0x06, 0x89, 0xE5]))
		self.assertGreater(gate, 0)
		self.assertIn(bytes([0x8E, 0x16]) + struct.pack("<H", 16), code1[gate:])  # mov ss, [__w16_ss16]
		far_into_3 = [r for r in segs[1]["relocs"] if r["src"] == "far" and r["target"]["seg"] == 1 and r["target"]["off"] == gate]
		self.assertTrue(far_into_3)  # the call into segment 3 goes through the gate

	def test_auto_fill_by_budget(self):
		e = Elf()
		e.section(".text.start", bytes(6) + RET, func="_start")
		for i in range(6):
			e.section(f".text.f{i}", b"\x90" * 999 + b"\xc3", func=f"f{i}")
		e.call(".text.start", 0, "f5")
		_o, info = ne_pack.pack(e.build(), module="T", segments={"segments": {"1": {"budget": 2100}}, "auto": {"budget": 2000}})
		self.assertEqual([s["number"] for s in info["segments"]], [1, 3, 4])
		self.assertEqual([s["units"] for s in info["segments"]], [3, 2, 2])

	def test_overfull_segments_are_split_again(self):
		e = Elf()
		e.section(".text.start", bytes(6) + RET, func="_start")
		for i in range(70):
			e.section(f".text.f{i}", b"\x90" * 999 + b"\xc3", func=f"f{i}")
		e.call(".text.start", 0, "f69")
		_o, info = ne_pack.pack(e.build(), module="T", segments={"auto": {"budget": 65535}})
		self.assertTrue(all(s["size"] <= 0xFFFF for s in info["segments"]))
		self.assertEqual(sum(s["units"] for s in info["segments"]), 71)

	def test_a_placed_segment_over_its_budget_fails(self):
		e = two_segments()
		cfg = {"segments": {"3": {"place": ["seg3.*"], "budget": 4}}}
		with self.assertRaisesRegex(ne_pack.PackError, "over its budget of 4"):
			ne_pack.pack(e.build(), module="T", segments=cfg)

	def test_plt32_calls_are_calls(self):
		e = two_segments()
		e.rels = [(sec, a, ne_pack.R_386_PLT32 if t == ne_pack.R_386_PC32 else t, sym) for sec, a, t, sym in e.rels]
		_o, info = ne_pack.pack(e.build(), module="T")
		self.assertEqual(info["veneers"], 2)

	def test_the_map_turns_a_fault_address_into_a_function(self):
		_o, info = ne_pack.pack(two_segments().build(), module="T")
		entries = [(n, off, size, name) for n, off, size, name in info["map"]]
		far_fn = next(o for s, o, _z, n in entries if n == "seg3.far_fn")
		self.assertEqual(ne_addr2sym.lookup(entries, 3, far_fn + 2), ("seg3.far_fn", 2, False))

	def test_refusals_name_the_symbol(self):
		e = two_segments()
		e.call(".text.start", 2, "far_fn", opcode=b"\x66\x05")  # an add with a relative operand: not a call or a jump
		with self.assertRaisesRegex(ne_pack.PackError, "far_fn"):
			ne_pack.pack(e.build(), module="T")
		e = two_segments()
		e.section(".text.taker", bytes(4) + RET, func="taker")
		e.pointer(".text.taker", 0, "far_fn", addend=2)  # an address inside far_fn, used as code from segment 1
		with self.assertRaisesRegex(ne_pack.PackError, "far_fn.*inside a function"):
			ne_pack.pack(e.build(), module="T")
		e = two_segments()
		e.rels.append((".text.start", e.addr(".text.start") + 9, ne_pack.R_386_32, "missing_fn"))
		with self.assertRaisesRegex(ne_pack.PackError, "undefined symbol missing_fn"):
			ne_pack.pack(e.build(), module="T")

	def test_config_errors(self):
		e = two_segments().build()
		for cfg, msg in [
			({"segments": {"2": {}}}, "data segment"),
			({"segments": {"5": {"place": ["seg3.*"]}}}, "without gaps"),
			({"segments": {"1": {"preload": False}}}, "always fixed"),
			({"segments": {"3": {"colour": 1}}}, "takes only"),
			({"auto": {"budget": 0}}, "budget"),
			({"extra": 1}, "top level"),
		]:
			with self.subTest(cfg=cfg), self.assertRaisesRegex(ne_pack.PackError, msg):
				ne_pack.pack(e, module="T", segments=cfg)

	@unittest.skipUnless(shutil.which("objdump"), "needs objdump")
	def test_the_second_reader_agrees(self):
		e = two_segments()
		e.data += bytes(8)
		e.pointer(".data", 16, "far_fn")
		out, info = ne_pack.pack(e.build(), module="T")
		bad, branches, far = ne_check.check(out)
		self.assertEqual(bad, [])
		self.assertGreater(far, 0)
		# Point the rewritten call one byte further: it now lands inside the veneer's first instruction.
		broken = bytearray(out)
		seg1 = segments(out)[1]
		start = next(o for s, o, _z, n in info["map"] if s == 1 and n == "start")
		site = seg1["file_off"] + start + 4
		struct.pack_into("<i", broken, site, struct.unpack_from("<i", broken, site)[0] + 1)
		self.assertTrue(ne_check.check(bytes(broken))[0])


class StubTests(unittest.TestCase):
	def test_stubs_call_windows_on_the_16_bit_stack(self):
		lines = gen_imports.stub("KERNEL", 29, "Yield", "", "v")
		call = lines.index(".byte 0x9a")
		self.assertEqual(lines[call - 2 : call], ["mov ss, word ptr [__w16_ss16]", "movzx esp, sp"])
		after = lines[call + 4 :]
		self.assertEqual(after[:4], [".byte 0x66, 0xe8", ".long w16_sync_alias - . - 4", "mov ss, word ptr [__w16_ss32]", "movzx esp, sp"])


class DemangleTests(unittest.TestCase):
	def test_v0_and_legacy(self):
		self.assertEqual(demangle("_RNvMCsgVZ98Am4vGV_3w16NtB2_3Log3dec"), "w16::Log::dec")
		self.assertEqual(demangle("_RNvNtCsgVZ98Am4vGV_3w166wizard4show"), "w16::wizard::show")
		self.assertEqual(demangle("_RNvMCsgVZ98Am4vGV_3w16NtB2_3Log3hex.26"), "w16::Log::hex")
		self.assertEqual(demangle("_RINvCsbKH4JZHKHk1_6bigseg4workKm7b_EB2_"), "bigseg::work::<123>")
		self.assertEqual(demangle("_ZN3w163Log3dec17h0123456789abcdefE"), "w16::Log::dec")
		self.assertEqual(demangle("w16_main"), "w16_main")
		self.assertEqual(demangle("_RNvXBroken"), "_RNvXBroken")


if __name__ == "__main__":
	unittest.main()
