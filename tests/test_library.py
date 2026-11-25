#!/usr/bin/env python3
# Tests for the packer's library mode: a tiny hand-assembled DLL is packed from a manifest, and tools/ne_dump.py (an independent reader) checks the NE tables.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# usage: python3 -m unittest discover -s tests -v
#        W16_UPDATE_GOLDEN=1 python3 -m unittest discover -s tests   rewrites tests/golden/tiny.dll after a deliberate format change
import hashlib
import json
import os
import struct
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "tools"))
sys.path.insert(0, HERE)

import ne_dump  # noqa: E402
import ne_pack  # noqa: E402
import test_ne_pack  # noqa: E402
import test_segments  # noqa: E402

GOLDEN = os.path.join(HERE, "golden", "tiny.dll")
MANIFEST = os.path.join(HERE, "..", "examples", "library", "exports.json")

# x86-16, all of it far-returning.
LIB_ENTRY = bytes([0xB8, 0x01, 0x00, 0xCB])  # mov ax, 1 ; retf: the load succeeds
ANSWER = bytes([0xB8, 0x2A, 0x00, 0xCB])  # mov ax, 42 ; retf: returns a constant
WEP = bytes([0xB8, 0x01, 0x00, 0xCA, 0x02, 0x00])  # mov ax, 1 ; retf 2
READ_DATA = bytes([0xB8, 0xFF, 0xFF, 0x8E, 0xD8, 0xA1, 0x10, 0x00, 0xCA, 0x02, 0x00])  # mov ax, <DGROUP> ; mov ds, ax ; mov ax, [10h] ; retf 2


def manifest():
	with open(MANIFEST, encoding="utf-8") as f:
		return json.load(f)


def tiny_elf(wep=True, extra_exports=(), imports=()):
	e = test_segments.Elf(bytes(16) + struct.pack("<H", 0x1234))
	e.section(".text.start", LIB_ENTRY, func="LibEntry")
	e.section(".text.exp_Answer", ANSWER, func="__w16exp_Answer")
	e.section(".text.exp_ReadData", READ_DATA, func="__w16exp_ReadData")
	e.symbol("__w16sel_data_0", ".text.exp_ReadData", e.addr(".text.exp_ReadData") + 1)
	if wep:
		e.section(".text.exp_WEP", WEP, func="__w16exp_WEP")
	for name in extra_exports:
		e.section(f".text.exp_{name}", ANSWER, func=f"__w16exp_{name}")
	for mod, ordinal, name in imports:
		e.section(f".text.imp_{name}", bytes(4), func=f"__w16imp_{mod}_{ordinal}_{name}")
	return e


def tiny_dll(**kw):
	return ne_pack.pack(tiny_elf().build(), library=manifest(), **kw)


class LibraryTests(unittest.TestCase):
	def test_header_of_a_library(self):
		out, info = tiny_dll()
		h = ne_dump.parse(out)["header"]
		self.assertEqual(h["flags"], 0x8301)  # library, uses the Windows API, single data
		self.assertEqual(h["autodata"], 2)
		self.assertEqual((h["cs"], h["ip"]), (1, 0))  # LibEntry is the first unit of segment 1
		self.assertEqual((h["ss"], h["sp"]), (0, 0))
		self.assertEqual(h["stack"], 0)
		self.assertEqual(h["heap"], 0)
		self.assertEqual(h["nseg"], 2)
		self.assertTrue(info["library"])
		self.assertEqual(info["ordinals"], {"WEP": 1, "Answer": 2, "ReadData": 4})

	def test_heap_comes_from_the_manifest(self):
		m = manifest()
		m["heap"] = 0x400
		out, _i = ne_pack.pack(tiny_elf().build(), library=m)
		self.assertEqual(ne_dump.parse(out)["header"]["heap"], 0x400)

	def test_data_segment_is_moveable(self):
		out, _i = tiny_dll()
		data = ne_dump.parse(out)["segments"][1]
		self.assertEqual(data["flags"], 0x0C51)  # data, moveable, preload, ring 3
		self.assertEqual(data["length"], 18)

	def test_entry_table_has_fixed_ordinals_and_a_gap(self):
		out, _i = tiny_dll()
		info = ne_dump.parse(out)
		h = info["header"]
		code = out[info["segments"][0]["file_off"] :]
		got = [(e["ordinal"], e["seg"], e["flags"], code[e["off"] : e["off"] + 2]) for e in info["entries"]]
		self.assertEqual([g[:3] for g in got], [(1, 1, 0x01), (2, 1, 0x01), (4, 1, 0x09)])  # ReadData: exported, one parameter word
		self.assertEqual([g[3] for g in got], [WEP[:2], ANSWER[:2], READ_DATA[:2]])
		raw = out[info["ne_offset"] + h["entry_off"] :][: h["entry_len"]]
		# A bundle of WEP and Answer in segment 1, one unused ordinal (3), a bundle of ReadData, and the end.
		self.assertEqual(raw[0:2] + raw[8:10] + raw[10:12] + raw[15:], bytes([2, 1, 1, 0, 1, 1, 0, 0]))

	def test_resident_names_carry_the_ordinals(self):
		out, _i = tiny_dll()
		info = ne_dump.parse(out)
		self.assertEqual(info["resident_names"], [("TINY", 0), ("WEP", 1), ("ANSWER", 2), ("READDATA", 4)])
		self.assertEqual(info["nonresident_names"], [("A hand-assembled test library", 0)])

	def test_ordinals_do_not_depend_on_manifest_order(self):
		m = manifest()
		m["exports"] = dict(reversed(list(m["exports"].items())))
		self.assertEqual(ne_pack.pack(tiny_elf().build(), library=m)[0], tiny_dll()[0])

	def test_data_selector_fixup_and_no_imports(self):
		out, _i = tiny_dll()
		info = ne_dump.parse(out)
		self.assertEqual(info["modules"], [])
		relocs = info["segments"][0]["relocs"]
		self.assertEqual(len(relocs), 1)
		r = relocs[0]
		self.assertEqual((r["src"], r["kind"], r["target"]), ("selector16", "internal", {"seg": 2, "off": 0}))
		code = out[info["segments"][0]["file_off"] :]
		self.assertEqual(code[r["offset"] : r["offset"] + 2], b"\xff\xff")  # the end of a chain of one
		self.assertEqual(code[r["offset"] - 1], 0xB8)  # the operand of mov ax, imm16

	def test_the_default_wep_succeeds_and_does_nothing(self):
		out, _i = ne_pack.pack(tiny_elf(wep=False).build(), library=manifest())
		info = ne_dump.parse(out)
		wep = next(e for e in info["entries"] if e["ordinal"] == 1)
		code = out[info["segments"][0]["file_off"] :]
		self.assertEqual(wep["seg"], 1)
		self.assertEqual(code[wep["off"] : wep["off"] + len(WEP)], WEP)
		self.assertIn(("WEP", 1), info["resident_names"])

	def test_wep_is_added_to_a_manifest_that_omits_it(self):
		m = manifest()
		del m["exports"]["WEP"]
		out, _i = ne_pack.pack(tiny_elf(wep=False).build(), library=m)
		self.assertIn(("WEP", 1), ne_dump.parse(out)["resident_names"])
		out, _i = ne_pack.pack(tiny_elf().build(), library=m)  # and a program's own WEP takes the same ordinal
		self.assertIn(("WEP", 1), ne_dump.parse(out)["resident_names"])

	def test_exe_mode_is_unchanged(self):
		text = bytes(range(64)) + b"\x90" * 64
		syms = {
			"_start": 0,
			"__w16imp_KERNEL_91_InitTask": 6,
			"__w16imp_USER_1_MessageBox": 20,
			"__w16imp_GDI_33_TextOut": 30,
			"__w16imp_USER_5_InitApp": 40,
			"__w16exp_WndProc": 50,
			"__w16exp_AboutDlg": 60,
			"__w16sel_data_0": 70,
		}
		out, _i = ne_pack.pack(test_ne_pack.make_elf(text, bytes(16) + b"hello\0", syms), module="TEST", description="A test")
		self.assertEqual(hashlib.sha256(out).hexdigest(), "7e302b3d5bbcfadcf5ebb56af2ccfea69809bde9d6b7208b873a2488bba545ba")
		out, _i = ne_pack.pack(test_segments.two_segments().build(), module="TEST")
		self.assertEqual(hashlib.sha256(out).hexdigest(), "3a97db83913101dc6921c828db9c473187c19abb9d354d9007c6d978468d69da")

	def test_calls_between_the_librarys_own_segments_get_veneers(self):
		e = test_segments.two_segments()
		e.syms = [("LibEntry", s[1], s[2], s[3]) if s[0] == "_start" else s for s in e.syms]
		out, _i = ne_pack.pack(e.build(), library={"module": "TWO", "exports": {}})
		info = ne_dump.parse(out)
		self.assertEqual(info["header"]["nseg"], 3)  # segment 1, the data and segment 3
		self.assertEqual(info["header"]["flags"], 0x8301)
		fars = [r for s in info["segments"] for r in s["relocs"] if r["src"] == "far"]
		self.assertGreaterEqual(len(fars), 2)  # a veneer each way, both internal
		self.assertTrue(all(r["kind"] == "internal" for r in fars))

	def test_golden_bytes(self):
		out, _i = tiny_dll()
		if os.environ.get("W16_UPDATE_GOLDEN") == "1":
			with open(GOLDEN, "wb") as f:
				f.write(out)
		with open(GOLDEN, "rb") as f:
			self.assertEqual(out, f.read())

	def test_command_line_packs_a_library(self):
		with tempfile.TemporaryDirectory() as d:
			elf, dll = os.path.join(d, "tiny.elf"), os.path.join(d, "TINY.DLL")
			with open(elf, "wb") as f:
				f.write(tiny_elf().build())
			r = subprocess.run([sys.executable, "-I", os.path.join(HERE, "..", "tools", "ne_pack.py"), "--library", MANIFEST, elf, dll], capture_output=True, text=True)
			self.assertEqual(r.returncode, 0, r.stderr)
			self.assertIn("library, exports WEP @1, Answer @2, ReadData @4", r.stdout)
			with open(dll, "rb") as f:
				self.assertEqual(f.read(), tiny_dll()[0])


class LibraryRefusalTests(unittest.TestCase):
	def refuses(self, elf, manifest_, text, **kw):
		with self.assertRaises(ne_pack.PackError) as cm:
			ne_pack.pack(elf.build(), library=manifest_, **kw)
		self.assertIn(text, str(cm.exception))

	def test_a_user_import_is_rejected(self):
		self.refuses(tiny_elf(imports=[("USER", 1, "MessageBox")]), manifest(), "USER.MessageBox is imported by a library")

	def test_a_kernel_import_off_the_default_list_is_rejected(self):
		m = manifest()
		del m["imports"]
		self.refuses(tiny_elf(imports=[("KERNEL", 29, "Yield")]), m, "KERNEL.Yield")

	def test_an_allowed_import_is_packed(self):
		out, i = ne_pack.pack(tiny_elf(imports=[("KERNEL", 15, "GlobalAlloc")]).build(), library=manifest())
		self.assertEqual(ne_dump.parse(out)["modules"], ["KERNEL"])
		self.assertEqual(i["imports"], 1)

	def test_the_manifest_can_widen_or_empty_the_list(self):
		m = manifest()
		m["imports"] = ["KERNEL.Yield"]
		ne_pack.pack(tiny_elf(imports=[("KERNEL", 29, "Yield")]).build(), library=m)
		m["imports"] = []
		self.refuses(tiny_elf(imports=[("KERNEL", 15, "GlobalAlloc")]), m, "may call only nothing")

	def test_a_duplicate_ordinal_is_rejected(self):
		m = manifest()
		m["exports"]["Answer"] = 4
		self.refuses(tiny_elf(), m, "share ordinal 4")

	def test_a_name_twice_in_the_manifest_is_rejected(self):
		with tempfile.TemporaryDirectory() as d:
			p = os.path.join(d, "dup.json")
			with open(p, "w", encoding="utf-8") as f:
				f.write('{"module": "X", "exports": {"A": 2, "A": 3}}')
			with self.assertRaises(ne_pack.PackError):
				ne_pack.load_library(p)

	def test_gaps_can_be_an_error(self):
		m = manifest()
		m["gaps"] = "error"
		self.refuses(tiny_elf(), m, "ordinal 3 is unused")
		m["exports"]["ReadData"] = 3
		ne_pack.pack(tiny_elf().build(), library=m)

	def test_an_export_missing_from_the_program_or_the_manifest(self):
		m = manifest()
		m["exports"]["Missing"] = 9
		self.refuses(tiny_elf(), m, "no __w16exp_Missing")
		self.refuses(tiny_elf(extra_exports=["Extra"]), manifest(), "__w16exp_Extra is not in the manifest")

	def test_wep_cannot_take_an_ordinal_that_is_in_use(self):
		m = manifest()
		del m["exports"]["WEP"]
		m["exports"]["Answer"] = 1
		self.refuses(tiny_elf(wep=False), m, "share ordinal 1")

	def test_an_oversized_segment_is_rejected(self):
		e = test_segments.Elf(bytes(16))
		e.section(".text.start", b"\x90" * 0x10001, func="LibEntry")
		self.refuses(e, {"module": "BIG", "exports": {}}, "over one 64 KB segment")
		e = test_segments.Elf(bytes(0xFFF0))
		e.section(".text.start", test_segments.RET, func="LibEntry")
		self.refuses(e, {"module": "BIG", "exports": {}, "heap": 0x100}, "over one 64 KB segment")

	def test_a_library_needs_libentry_and_a_name(self):
		e = test_segments.Elf()
		e.section(".text.start", test_segments.RET, func="_start")
		self.refuses(e, {"module": "X", "exports": {}}, "no LibEntry")
		self.refuses(tiny_elf(), {"exports": {}}, "needs a module name")

	def test_a_library_has_no_stack_setting(self):
		self.refuses(tiny_elf(), manifest(), "no stack of its own", stack=0x2000)

	def test_bad_manifests(self):
		for bad, text in [
			([], "must be a JSON object"),
			({"module": "X"}, "needs an exports object"),
			({"module": "X", "exports": {"A": 0}}, "not between 1 and 32767"),
			({"module": "X", "exports": {"A": True}}, "must be an ordinal or an object"),
			({"module": "X", "exports": {"A": {"ordinal": 2, "wrods": 1}}}, "must be an ordinal or an object"),
			({"module": "X", "exports": {}, "colour": 1}, "unknown key"),
			({"module": "X", "exports": {}, "imports": ["Yield"]}, "MODULE.Name"),
		]:
			with self.assertRaises(ne_pack.PackError) as cm:
				ne_pack.check_library(bad)
			self.assertIn(text, str(cm.exception))


if __name__ == "__main__":
	unittest.main()
