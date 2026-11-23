#!/usr/bin/env python3
# Reads a Win16 NE executable and prints every table it can: the independent check on tools/ne_pack.py, and the way to look at a reference EXE.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# usage: tools/ne_dump.py [--json] file.exe
import json
import struct
import sys

RELOC_SRC = {0: "lobyte", 1: "selector", 2: "selector16", 3: "far", 5: "offset16", 11: "offset32", 13: "offset32b"}
RELOC_KIND = {0: "internal", 1: "ordinal", 2: "name", 3: "osfixup"}


def cstr_pascal(data, off):
	n = data[off]
	return data[off + 1 : off + 1 + n].decode("latin-1"), off + 1 + n


def parse(data):
	if data[:2] != b"MZ":
		raise ValueError("not an MZ executable")
	ne = struct.unpack_from("<I", data, 0x3C)[0]
	if data[ne : ne + 2] != b"NE":
		raise ValueError("no NE header")
	u16 = lambda o: struct.unpack_from("<H", data, ne + o)[0]
	u32 = lambda o: struct.unpack_from("<I", data, ne + o)[0]
	h = {
		"linker_ver": data[ne + 2],
		"linker_rev": data[ne + 3],
		"entry_off": u16(4),
		"entry_len": u16(6),
		"crc": u32(8),
		"flags": u16(0x0C),
		"autodata": u16(0x0E),
		"heap": u16(0x10),
		"stack": u16(0x12),
		"ip": u16(0x14),
		"cs": u16(0x16),
		"sp": u16(0x18),
		"ss": u16(0x1A),
		"nseg": u16(0x1C),
		"nmod": u16(0x1E),
		"nonres_size": u16(0x20),
		"seg_off": u16(0x22),
		"res_off": u16(0x24),
		"resname_off": u16(0x26),
		"modref_off": u16(0x28),
		"impname_off": u16(0x2A),
		"nonres_off": u32(0x2C),
		"nmovable": u16(0x30),
		"shift": u16(0x32),
		"nres": u16(0x34),
		"target_os": data[ne + 0x36],
		"other_flags": data[ne + 0x37],
		"gangload_off": u16(0x38),
		"gangload_len": u16(0x3A),
		"swap_area": u16(0x3C),
		"expected_ver": u16(0x3E),
	}
	out = {"ne_offset": ne, "header": h}
	shift = h["shift"] or 9

	segs = []
	for i in range(h["nseg"]):
		off, ln, fl, mn = struct.unpack_from("<HHHH", data, ne + h["seg_off"] + 8 * i)
		seg = {"index": i + 1, "file_off": off << shift if off else 0, "length": ln or (0x10000 if off else 0), "flags": fl, "minalloc": mn or 0x10000}
		relocs = []
		if fl & 0x100:
			start = seg["file_off"] + (ln or 0x10000)
			n = struct.unpack_from("<H", data, start)[0]
			for r in range(n):
				src, kind, roff, a, b = struct.unpack_from("<BBHHH", data, start + 2 + 8 * r)
				rec = {"src": RELOC_SRC.get(src & 0x0F, src), "kind": RELOC_KIND.get(kind & 3, kind), "additive": bool(kind & 4), "offset": roff}
				if kind & 3 == 0:
					rec["target"] = {"seg": a & 0xFF, "off": b} if (a & 0xFF) != 0xFF else {"entry": b}
				elif kind & 3 == 1:
					rec["module"] = a
					rec["ordinal"] = b
				elif kind & 3 == 2:
					rec["module"] = a
					rec["name_off"] = b
				else:
					rec["osfixup"] = a
				relocs.append(rec)
		seg["relocs"] = relocs
		segs.append(seg)
	out["segments"] = segs

	mods = []
	for i in range(h["nmod"]):
		o = struct.unpack_from("<H", data, ne + h["modref_off"] + 2 * i)[0]
		mods.append(cstr_pascal(data, ne + h["impname_off"] + o)[0])
	out["modules"] = mods

	def name_table(off):
		res = []
		while data[off]:
			name, off = cstr_pascal(data, off)
			(ordv,) = struct.unpack_from("<H", data, off)
			off += 2
			res.append((name, ordv))
		return res

	out["resident_names"] = name_table(ne + h["resname_off"])
	out["nonresident_names"] = name_table(h["nonres_off"]) if h["nonres_size"] else []

	entries = []
	o = ne + h["entry_off"]
	end = o + h["entry_len"]
	ordv = 1
	while o < end:
		cnt = data[o]
		if cnt == 0:
			break
		ind = data[o + 1]
		o += 2
		for _ in range(cnt):
			if ind == 0:
				pass
			elif ind == 0xFF:
				fl, _int3f, seg, off = struct.unpack_from("<BHBH", data, o)
				entries.append({"ordinal": ordv, "flags": fl, "seg": seg, "off": off, "movable": True})
				o += 6
			else:
				fl, off = struct.unpack_from("<BH", data, o)
				entries.append({"ordinal": ordv, "flags": fl, "seg": ind, "off": off, "movable": False})
				o += 3
			ordv += 1
	out["entries"] = entries

	res = {"shift": None, "types": []}
	if h["res_off"] != h["resname_off"]:
		o = ne + h["res_off"]
		rshift = struct.unpack_from("<H", data, o)[0]
		res["shift"] = rshift
		o += 2
		while True:
			(tid,) = struct.unpack_from("<H", data, o)
			if tid == 0:
				break
			cnt = struct.unpack_from("<H", data, o + 2)[0]
			o += 8
			items = []
			for _ in range(cnt):
				roff, rlen, rfl, rid, _hnd, _use = struct.unpack_from("<HHHHHH", data, o)
				items.append({"id": rid & 0x7FFF if rid & 0x8000 else rid, "file_off": roff << rshift, "length": rlen << rshift, "flags": rfl})
				o += 12
			res["types"].append({"type": tid, "items": items})
	out["resources"] = res
	return out


def main():
	args = sys.argv[1:]
	as_json = False
	if args[:1] == ["--json"]:
		as_json = True
		args = args[1:]
	if len(args) != 1:
		sys.exit(__doc__)
	with open(args[0], "rb") as f:
		info = parse(f.read())
	if as_json:
		print(json.dumps(info, indent=1))
		return
	h = info["header"]
	print("NE header:")
	for k, v in h.items():
		print(f"  {k:14} {v:#x}" if isinstance(v, int) else f"  {k:14} {v}")
	print("modules:", info["modules"])
	print("resident names:", info["resident_names"])
	print("non-resident names:", info["nonresident_names"])
	print("entries:")
	for e in info["entries"]:
		print("  ", e)
	for s in info["segments"]:
		print(f"segment {s['index']}: file_off={s['file_off']:#x} len={s['length']:#x} flags={s['flags']:#06x} minalloc={s['minalloc']:#x}")
		for r in s["relocs"]:
			print("   ", r)
	print("resources:", json.dumps(info["resources"]))


if __name__ == "__main__":
	main()
