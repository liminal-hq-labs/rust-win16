#!/usr/bin/env python3
# Turns a segment:offset from a General Protection Fault box into the function it points into, from the map `ne_pack.py --map` wrote.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# usage: tools/ne_addr2sym.py PROGRAM.MAP SEG:OFFSET [SEG:OFFSET ...]      (hexadecimal, as Windows shows it: 0003:11D0)
#
# Taken from Quill's win16/tools/ne_addr2sym.py, which reads the same map format.
import sys


def load(path):
	entries = []
	with open(path, encoding="utf-8") as f:
		for line in f:
			loc, size, name = line.split(None, 2)
			seg, off = (int(x, 16) for x in loc.split(":"))
			entries.append((seg, off, int(size), name.strip()))
	return entries


def lookup(entries, seg, off):
	"""Returns the name and the offset into it of the entry holding seg:off, or None."""
	best = None
	for s, o, size, name in entries:
		if s == seg and o <= off and (best is None or o > best[0]):
			best = (o, size, name)
	if best is None:
		return None
	o, size, name = best
	return name, off - o, off >= o + max(size, 1)


def main():
	if len(sys.argv) < 3:
		sys.exit("usage: ne_addr2sym.py PROGRAM.MAP SEG:OFFSET [SEG:OFFSET ...]")
	entries = load(sys.argv[1])
	for arg in sys.argv[2:]:
		seg, off = (int(x, 16) for x in arg.split(":"))
		hit = lookup(entries, seg, off)
		if hit is None:
			print(f"{arg}: no symbol")
		else:
			name, into, past = hit
			print(f"{arg}: {name} + {into:#x}" + (" (past its end)" if past else ""))


if __name__ == "__main__":
	main()
