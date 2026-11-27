#!/usr/bin/env python3
# Tests for the library runtime's tooling: dynamic import rows in tools/gen_imports.py, the committed src/imports.rs, and the hello library's manifest.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# usage: python3 -m unittest discover -s tests -v
import json
import os
import re
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..")
sys.path.insert(0, os.path.join(ROOT, "tools"))

import gen_imports  # noqa: E402
import ne_pack  # noqa: E402


def table_of(text):
	with tempfile.NamedTemporaryFile("w", suffix=".tbl", delete=False, encoding="utf-8") as f:
		f.write(text)
	try:
		return gen_imports.parse_table(f.name)
	finally:
		os.unlink(f.name)


class DynamicImports(unittest.TestCase):
	def test_dyn_row_is_parsed_apart_from_a_loader_import(self):
		rows = table_of("KERNEL 29 Yield - v\nDYN LIB 2 LibInfo p w\n")
		self.assertEqual(rows[0][0], "KERNEL")
		self.assertEqual(rows[1], ("DYN:LIB", 2, "LibInfo", "p", "w"))

	def test_bad_dyn_rows_are_refused(self):
		for bad in ("DYN LIB 2 LibInfo p\n", "DYN LIB 2 LibInfo q w\n", "DYN LIB 2 LibInfo p x\n"):
			with self.assertRaises(SystemExit):
				table_of(bad)

	def test_stub_calls_through_a_slot_and_reports_a_missing_library(self):
		lines = gen_imports.stub("DYN:LIB", 3, "LibAdd", "ww", "l")
		text = "\n".join(lines)
		# No import label: the loader must not see the function, since the program loads the library itself.
		self.assertNotIn("__w16imp_", text)
		self.assertIn("cmp word ptr [__w16dyn_LibAdd+2], 0", lines)
		self.assertIn(".byte 0xff, 0x1e", lines)  # call far [slot]
		self.assertIn(".word __w16dyn_LibAdd", lines)
		self.assertIn("add sp, 4", lines)  # the two words are dropped when the slot is empty
		self.assertIn("mov ax, 0xffff", lines)
		self.assertLess(lines.index("mov ax, 0xffff"), lines.index(".long w16_sync_alias - . - 4"))  # the alias sync follows both paths

	def test_dropped_words_count_pointers_and_longs_twice(self):
		lines = gen_imports.stub("DYN:LIB", 4, "LibFill", "lww", "w")
		self.assertIn("add sp, 8", lines)

	def test_render_adds_the_slot_the_ordinal_and_the_binder(self):
		out = gen_imports.render([("DYN:LIB", 9, "LibOne", "", "w")])
		self.assertIn('"__w16dyn_LibOne: .long 0"', out)
		self.assertIn("pub const LibOne_ORDINAL: u32 = 9;", out)
		self.assertIn("pub fn bind_LibOne(far: u32)", out)

	def test_spec_check_skips_a_dynamic_row(self):
		self.assertEqual(gen_imports.check_spec([("DYN:LIB", 2, "LibInfo", "p", "w")], "/nonexistent"), 0)

	def test_committed_imports_match_the_table(self):
		rows = gen_imports.parse_table(os.path.join(ROOT, "tools", "imports.tbl"))
		with open(os.path.join(ROOT, "src", "imports.rs"), encoding="utf-8") as f:
			self.assertEqual(f.read(), gen_imports.render(rows), "run scripts/build.sh --regen")

	def test_the_dynamic_rows_name_the_hello_libraries_ordinals(self):
		with open(os.path.join(ROOT, "examples", "hellodll", "exports.json"), encoding="utf-8") as f:
			exports = json.load(f)["exports"]
		ordinals = {k: (v if isinstance(v, int) else v["ordinal"]) for k, v in exports.items()}
		rows = [r for r in gen_imports.parse_table(os.path.join(ROOT, "tools", "imports.tbl")) if r[0] == "DYN:HELLODLL"]
		self.assertEqual(len(rows), len(exports) - 1)  # every export but WEP
		for _mod, ordinal, name, _args, _ret in rows:
			self.assertEqual(ordinals[name.removeprefix("Hello")], ordinal, name)


class HelloManifest(unittest.TestCase):
	def test_manifest_is_accepted_and_has_no_gaps(self):
		with open(os.path.join(ROOT, "examples", "hellodll", "exports.json"), encoding="utf-8") as f:
			lib = ne_pack.check_library(json.load(f))
		self.assertEqual(sorted(o for o, _w, _s in lib["exports"].values()), list(range(1, 11)))
		self.assertEqual(lib["imports"], set())  # the library calls nothing in Windows

	def test_words_in_the_manifest_match_the_thunks_in_the_source(self):
		with open(os.path.join(ROOT, "examples", "hellodll", "exports.json"), encoding="utf-8") as f:
			exports = json.load(f)["exports"]
		with open(os.path.join(ROOT, "examples", "hellodll", "main.rs"), encoding="utf-8") as f:
			source = f.read()
		thunks = dict((n, int(w)) for n, w in re.findall(r"export_dll!\((\w+), \w+, (\d+)\)", source))
		for name, entry in exports.items():
			if name in thunks:
				self.assertEqual(entry["words"], thunks[name], name)
		# The runtime defines WEP; DsSel is a few instructions of assembly in the example.
		self.assertEqual(set(exports) - set(thunks), {"WEP", "DsSel"})


if __name__ == "__main__":
	unittest.main()
