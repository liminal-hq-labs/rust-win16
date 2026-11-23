#!/usr/bin/env python3
# Tests for tools/ne_pack.py: a synthetic ELF goes in, and tools/ne_dump.py (an independent reader) checks the NE tables that come out.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# usage: python3 -m unittest discover -s tests -v
import os
import struct
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tools"))

import gen_imports  # noqa: E402
import ne_dump  # noqa: E402
import ne_pack  # noqa: E402


def make_elf(text, data, symbols, bss=0):
	"""Builds a minimal 32-bit x86 ELF with .text, .data (and optionally .bss), a symbol table and no program headers.

	`symbols` maps a name to its offset in .text."""
	shstr = b"\0.text\0.data\0.bss\0.symtab\0.strtab\0.shstrtab\0"

	def name_of(n):
		return shstr.index(b"\0" + n.encode() + b"\0") + 1

	strtab = b"\0"
	symtab = bytes(16)
	for nm, val in symbols.items():
		symtab += struct.pack("<IIIBBH", len(strtab), val, 0, 0x10, 0, 1)
		strtab += nm.encode() + b"\0"
	off = 52
	text_off = off
	data_off = text_off + len(text)
	symtab_off = data_off + len(data)
	strtab_off = symtab_off + len(symtab)
	shstr_off = strtab_off + len(strtab)
	sh_off = shstr_off + len(shstr)
	secs = [
		(0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
		(name_of(".text"), 1, 6, 0, text_off, len(text), 0, 0, 1, 0),
		(name_of(".data"), 1, 3, 0, data_off, len(data), 0, 0, 1, 0),
		(name_of(".symtab"), 2, 0, 0, symtab_off, len(symtab), 4, 0, 4, 16),
		(name_of(".strtab"), 3, 0, 0, strtab_off, len(strtab), 0, 0, 1, 0),
		(name_of(".shstrtab"), 3, 0, 0, shstr_off, len(shstr), 0, 0, 1, 0),
	]
	if bss:
		secs.append((name_of(".bss"), 8, 3, 0, 0, bss, 0, 0, 1, 0))
	hdr = b"\x7fELF" + bytes([1, 1, 1, 0]) + bytes(8)
	hdr += struct.pack("<HHIIIIIHHHHHH", 2, 3, 1, 0, 0, sh_off, 0, 52, 0, 0, 40, len(secs), 5)
	body = text + data + symtab + strtab + shstr
	for s in secs:
		body += struct.pack("<IIIIIIIIII", *s)
	return hdr + body


class PackTests(unittest.TestCase):
	def pack(self, **kw):
		text = bytes(range(64)) + b"\x90" * 64
		data = bytes(16) + b"hello\0"
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
		out, info = ne_pack.pack(make_elf(text, data, syms), module="TEST", description="A test", **kw)
		return text, data, out, info

	def test_mz_stub_and_ne_header(self):
		_t, _d, out, _i = self.pack()
		self.assertEqual(out[:2], b"MZ")
		self.assertEqual(struct.unpack_from("<I", out, 0x3C)[0], 0x80)
		self.assertEqual(out[0x80:0x82], b"NE")
		self.assertIn(b"This program requires Microsoft Windows.", out[:0x80])
		h = ne_dump.parse(out)["header"]
		self.assertEqual(h["flags"], 0x0302)  # uses the Windows API, multiple data segments
		self.assertEqual(h["nseg"], 2)
		self.assertEqual(h["autodata"], 2)
		self.assertEqual((h["cs"], h["ip"]), (1, 0))
		self.assertEqual((h["ss"], h["sp"]), (2, 0))  # SP of 0: the loader puts the stack above the data
		self.assertEqual(h["stack"], 0x2000)
		self.assertEqual(h["heap"], 0x1000)
		self.assertEqual(h["target_os"], 2)
		self.assertEqual(h["shift"], ne_pack.FILE_SHIFT)
		self.assertEqual(h["expected_ver"], 0x030A)

	def test_segment_table(self):
		text, data, out, _i = self.pack()
		info = ne_dump.parse(out)
		code, dat = info["segments"]
		self.assertEqual(code["length"], len(text))
		self.assertEqual(code["flags"], 0x0D40)  # preload, relocations, ring 3, fixed, code
		self.assertEqual(dat["length"], len(data))
		self.assertEqual(dat["flags"], 0x0C41)  # data, preload, ring 3
		self.assertEqual(code["file_off"] % 16, 0)
		self.assertEqual(out[code["file_off"] : code["file_off"] + len(text)][:6], text[:6])
		self.assertEqual(out[dat["file_off"] : dat["file_off"] + len(data)], data)

	def test_import_fixups(self):
		_t, _d, out, i = self.pack()
		info = ne_dump.parse(out)
		self.assertEqual(info["modules"], ["KERNEL", "USER", "GDI"])
		relocs = {r["offset"]: r for r in info["segments"][0]["relocs"]}
		# far-address (type 3) imported-ordinal (kind 1) records, module numbers are 1-based indices into the module table
		self.assertEqual((relocs[6]["src"], relocs[6]["kind"], relocs[6]["module"], relocs[6]["ordinal"]), ("far", "ordinal", 1, 91))
		self.assertEqual((relocs[20]["module"], relocs[20]["ordinal"]), (2, 1))
		self.assertEqual((relocs[30]["module"], relocs[30]["ordinal"]), (3, 33))
		self.assertEqual((relocs[40]["module"], relocs[40]["ordinal"]), (2, 5))
		self.assertEqual(i["imports"], 4)

	def test_fixup_sites_end_the_chain(self):
		# A non-additive fixup is a chain link: the loader reads the next offset from the site, and 0xFFFF ends it.
		_t, _d, out, _i = self.pack()
		code = ne_dump.parse(out)["segments"][0]
		base = code["file_off"]
		for r in code["relocs"]:
			self.assertEqual(struct.unpack_from("<H", out, base + r["offset"])[0], 0xFFFF, r)

	def test_relocation_record_encoding(self):
		_t, _d, out, _i = self.pack()
		code = ne_dump.parse(out)["segments"][0]
		start = code["file_off"] + code["length"]
		(n,) = struct.unpack_from("<H", out, start)
		self.assertEqual(n, 5)  # four imports and one selector fixup
		records = [struct.unpack_from("<BBHHH", out, start + 2 + 8 * k) for k in range(n)]
		self.assertEqual(records[0], (3, 1, 6, 1, 91))
		sel = [r for r in records if r[0] == 2][0]
		self.assertEqual(sel, (2, 0, 70, 2, 0))  # 16-bit selector, internal reference, to segment 2
		self.assertEqual([r[2] for r in records], sorted(r[2] for r in records))

	def test_entry_table_and_names(self):
		_t, _d, out, i = self.pack()
		info = ne_dump.parse(out)
		self.assertEqual(i["exports"], ["AboutDlg", "WndProc"])
		self.assertEqual([(e["ordinal"], e["seg"], e["off"], e["flags"]) for e in info["entries"]], [(1, 1, 60, 3), (2, 1, 50, 3)])
		self.assertEqual(info["resident_names"], [("TEST", 0), ("ABOUTDLG", 1), ("WNDPROC", 2)])
		self.assertEqual(info["nonresident_names"], [("A test", 0)])

	def test_no_exports_leaves_an_empty_entry_table(self):
		elf = make_elf(b"\x90" * 8, bytes(16), {"_start": 0})
		out, _i = ne_pack.pack(elf, module="X")
		info = ne_dump.parse(out)
		self.assertEqual(info["entries"], [])
		self.assertEqual(info["header"]["entry_len"], 2)
		self.assertEqual(info["modules"], [])

	def test_bss_is_folded_into_the_data_segment(self):
		elf = make_elf(b"\x90" * 8, bytes(16), {"_start": 0}, bss=100)
		out, info = ne_pack.pack(elf, module="X")
		self.assertEqual(info["data"], 116)

	def test_size_limits(self):
		with self.assertRaises(ne_pack.PackError):
			ne_pack.pack(make_elf(b"\x90" * 0x10001, bytes(16), {"_start": 0}), module="X")
		with self.assertRaises(ne_pack.PackError):
			ne_pack.pack(make_elf(b"\x90", bytes(0xF000), {"_start": 0}), module="X", stack=0x2000, heap=0x1000)
		with self.assertRaises(ne_pack.PackError):
			ne_pack.pack(make_elf(b"\x90", bytes(16), {}), module="X")

	def test_resources(self):
		dlg = {
			"id": 100,
			"caption": "Hi",
			"x": 1,
			"y": 2,
			"cx": 100,
			"cy": 50,
			"items": [{"class": "button", "id": 1, "text": "OK", "x": 5, "y": 6, "cx": 40, "cy": 14, "style": ["defpushbutton"]}],
		}
		res = [
			(ne_pack.RT_DIALOG, 100, ne_pack.dialog_template(dlg)),
			(ne_pack.RT_STRING, 1, ne_pack.string_blocks({"1": "one", "2": "two"})[1]),
			(ne_pack.RT_RCDATA, 7, struct.pack("<I", 5) + b"12345"),
		]
		elf = make_elf(b"\x90" * 8, bytes(16), {"_start": 0})
		out, _i = ne_pack.pack(elf, module="X", resources=res)
		info = ne_dump.parse(out)
		self.assertEqual(info["header"]["nres"], 3)
		self.assertEqual(info["resources"]["shift"], ne_pack.RES_SHIFT)
		types = {t["type"]: t["items"] for t in info["resources"]["types"]}
		self.assertEqual(sorted(types), [0x8000 | 5, 0x8000 | 6, 0x8000 | 10])
		item = types[0x8005][0]
		self.assertEqual(item["id"], 100)
		self.assertEqual(item["file_off"] % 16, 0)
		blob = out[item["file_off"] : item["file_off"] + len(res[0][2])]
		self.assertEqual(blob, res[0][2])
		self.assertGreaterEqual(item["length"], len(res[0][2]))  # lengths are in alignment units, so rounded up

	def test_dialog_template_layout(self):
		t = ne_pack.dialog_template(
			{"id": 1, "caption": "Cap", "x": 10, "y": 20, "cx": 30, "cy": 40, "items": [{"class": "static", "id": 9, "text": "T", "x": 1, "y": 2, "cx": 3, "cy": 4}]}
		)
		style, count = struct.unpack_from("<IB", t, 0)
		self.assertEqual(count, 1)
		self.assertTrue(style & ne_pack.WS["setfont"])
		self.assertEqual(struct.unpack_from("<4h", t, 5), (10, 20, 30, 40))
		self.assertEqual(t[13:14], b"\0")  # no menu
		self.assertEqual(t[14:15], b"\0")  # default class
		self.assertEqual(t[15:19], b"Cap\0")
		self.assertEqual(struct.unpack_from("<H", t, 19)[0], 8)  # font size
		pos = 19 + 2 + len(b"MS Sans Serif\0")
		self.assertEqual(struct.unpack_from("<4hH", t, pos), (1, 2, 3, 4, 9))
		self.assertEqual(t[pos + 10 + 4], 0x82)  # the stock STATIC class
		self.assertEqual(t[-1], 0)  # no creation data

	def test_string_blocks(self):
		blocks = ne_pack.string_blocks({"1": "a", "17": "bc"})
		self.assertEqual(sorted(blocks), [1, 2])  # string 1 is in block 1, string 17 in block 2
		self.assertEqual(blocks[1][:1], b"\0")  # string 0 is empty
		self.assertEqual(blocks[1][1:3], b"\x01a")


class GenImportsTests(unittest.TestCase):
	def test_table_parses_and_stubs_follow_the_pascal_convention(self):
		rows = gen_imports.parse_table(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tools", "imports.tbl"))
		names = {r[2]: r for r in rows}
		self.assertEqual(names["MessageBox"][:2], ("USER", 1))
		self.assertEqual(names["InitApp"][:2], ("USER", 5))
		lines = gen_imports.stub("USER", 1, "MessageBox", "wppw", "w")
		pushes = [ln for ln in lines if ln.startswith("push") and "[bp+" in ln]
		self.assertEqual(pushes, ["push word ptr [bp+8]", "push word ptr [bp+12]", "push word ptr [bp+16]", "push word ptr [bp+20]"])
		self.assertEqual(lines.count("push ds"), 3)  # two string arguments become DS:offset, and one restores ES
		self.assertIn("__w16imp_USER_1_MessageBox:", lines)

	def test_long_arguments_push_the_high_word_first(self):
		lines = gen_imports.stub("USER", 107, "DefWindowProc", "wwwl", "l")
		pushes = [ln for ln in lines if "[bp+" in ln]
		self.assertEqual(pushes[-2:], ["push word ptr [bp+22]", "push word ptr [bp+20]"])
		self.assertIn("shl edx, 16", lines)

	def test_callee_saved_registers_and_es_are_restored(self):
		lines = gen_imports.stub("KERNEL", 29, "Yield", "", "v")
		for reg in ("ebx", "esi", "edi"):
			self.assertIn(f"push {reg}", lines)
			self.assertIn(f"pop {reg}", lines)
		self.assertIn("movzx esp, sp", lines)
		self.assertEqual(lines[lines.index("pop es") - 1], "push ds")


if __name__ == "__main__":
	unittest.main()
