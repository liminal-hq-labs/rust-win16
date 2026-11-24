#!/usr/bin/env python3
# Decodes every code segment of a packed Win16 program and checks that no branch leaves its segment: the second reader for the packer's
# veneers.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# usage: tools/ne_check.py PROGRAM.EXE
#
# It does not trust the packer: it reads the file with ne_dump.py, disassembles each code segment with objdump (16-bit code, with the
# 66 and 67 prefixes the code16 target uses) and fails when a near call, jump or conditional branch targets an offset past the end of
# its segment or the middle of an instruction, or when an internal far fixup (a veneer's or trampoline's far jump) points at the middle of
# an instruction in its target segment. Needs objdump from GNU binutils.
import os
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ne_dump  # noqa: E402

BRANCH = re.compile(r"(?:data32\s+|addr32\s+)*(call[wl]?|jmp[wl]?|j[a-z]+|loop[a-z]*)\s+(0x[0-9a-f]+|[0-9a-f]+)\b")


def disassemble(code):
	"""Returns ({instruction start}, [(offset, mnemonic, target, text)]) for one code segment."""
	with tempfile.NamedTemporaryFile(suffix=".bin") as f:
		f.write(code)
		f.flush()
		out = subprocess.run(["objdump", "-D", "-b", "binary", "-m", "i8086", "-M", "intel", f.name], capture_output=True, text=True, check=True).stdout
	starts = set()
	branches = []
	for line in out.splitlines():
		m = re.match(r"\s*([0-9a-f]+):\t([0-9a-f ]+)\t(.*)", line)
		if not m:
			continue
		at = int(m.group(1), 16)
		starts.add(at)
		text = m.group(3).strip()
		b = BRANCH.match(text)
		if b and "far" not in text and "[" not in text and "ptr" not in text:
			branches.append((at, b.group(1), int(b.group(2), 16), text))
	return starts, branches


def check(data):
	"""Returns (problems, number of near branches checked, number of far fixups checked)."""
	info = ne_dump.parse(data)
	data_seg = info["header"]["autodata"]
	starts = {}
	bad = []
	total = 0
	segs = {s["index"]: s for s in info["segments"]}
	for n, s in segs.items():
		if n == data_seg or s["flags"] & 1:
			continue
		code = data[s["file_off"] : s["file_off"] + s["length"]]
		st, branches = disassemble(code)
		starts[n] = st
		for at, op, target, text in branches:
			total += 1
			if target >= s["length"]:
				bad.append(f"segment {n}: {at:04x}: {text}: the target {target:#x} is past the end of the segment ({s['length']:#x})")
			elif target not in st:
				bad.append(f"segment {n}: {at:04x}: {text}: the target lands inside an instruction")
	far = 0
	for n, s in segs.items():
		for r in s["relocs"]:
			t = r.get("target")
			if r["kind"] != "internal" or r["src"] != "far" or not t or "seg" not in t or t["seg"] not in starts:
				continue
			far += 1
			if t["off"] not in starts[t["seg"]]:
				bad.append(f"segment {n}: far fixup at {r['offset']:04x} points at {t['seg']}:{t['off']:04x}, inside an instruction")
	return bad, total, far


def main():
	if len(sys.argv) != 2:
		sys.exit("usage: ne_check.py PROGRAM.EXE")
	with open(sys.argv[1], "rb") as f:
		data = f.read()
	bad, total, far = check(data)
	if bad:
		sys.exit(f"ne_check: {sys.argv[1]}:\n" + "\n".join(bad[:20]) + (f"\n... and {len(bad) - 20} more" if len(bad) > 20 else ""))
	print(f"ne_check: ok: {sys.argv[1]}: {total} near branches stay in their segments and {far} far fixups land on instructions")


if __name__ == "__main__":
	main()
