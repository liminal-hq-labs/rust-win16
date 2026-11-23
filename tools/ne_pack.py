#!/usr/bin/env python3
# Packs a linked 32-bit x86 ELF built for the win16 target into a Win16 NE executable that Windows 3.x, 9x and Wine's krnl386 load.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# usage: tools/ne_pack.py [--module NAME] [--description TEXT] [--stack N] [--heap N] [--resources FILE.json] <in.elf> <out.exe>
#
# The linker has already resolved every symbol; this tool rearranges bytes and writes the tables Windows reads. It reads the ELF section
# headers, not the program headers: `.text` becomes code segment 1 (fixed, preloaded) and `.data` becomes the automatic data segment 2.
# The linker script (link.ld) puts both at offset 0 of their own segment. `.bss` is folded into `.data`.
#
# Symbols the packer reads from the ELF symbol table:
#   __w16imp_<MODULE>_<ordinal>_<Name>   a 4-byte far-call operand in `.text`: an imported-ordinal fixup (type "far address") is written for it
#   __w16exp_<Name>               the start of an exported far procedure: an entry-table entry and a resident name are written for it
#   __w16sel_code_<n>             a 16-bit word in `.text` that must hold the code segment's selector (selector fixup to segment 1)
#   __w16sel_data_<n>             a 16-bit word in `.text` that must hold the data segment's selector (selector fixup to segment 2)
#   _start                        the entry point (CS:IP in the header)
#
# Resources come from a JSON file (see scripts/build.sh and examples/*/resources.json): dialogs, strings and raw data, each with an integer ID. Raw data (RT_RCDATA)
# starts with its exact length as a little-endian 32-bit number.
import json
import os
import re
import struct
import sys

MZ_STUB_MESSAGE = b"This program requires Microsoft Windows.\r\n$"
SEG_DATA = 0x0001
SEG_PRELOAD = 0x0040
SEG_RELOCS = 0x0100
SEG_RING3 = 0x0C00
NE_MULTIPLEDATA = 0x0002
NE_WINAPI = 0x0300
RES_FLAGS = 0x1030  # MOVEABLE | PURE | DISCARDABLE
RES_SHIFT = 4
FILE_SHIFT = 4
RT_DIALOG = 5
RT_STRING = 6
RT_RCDATA = 10
CLASS_IDS = {"button": 0x80, "edit": 0x81, "static": 0x82, "listbox": 0x83, "scrollbar": 0x84, "combobox": 0x85}
WS = {
	"popup": 0x80000000,
	"child": 0x40000000,
	"visible": 0x10000000,
	"caption": 0x00C00000,
	"sysmenu": 0x00080000,
	"tabstop": 0x00010000,
	"group": 0x00020000,
	"border": 0x00800000,
	"modalframe": 0x80,
	"setfont": 0x40,
	"defpushbutton": 1,
	"pushbutton": 0,
	"center": 1,
	"left": 0,
	"right": 2,
}


class PackError(Exception):
	pass


def align(x, a):
	return (x + a - 1) & ~(a - 1)


def read_elf(data):
	if data[:4] != b"\x7fELF" or data[4] != 1 or data[5] != 1:
		raise PackError("not a 32-bit little-endian ELF file")
	e_type, e_machine, _v, e_entry, _phoff, e_shoff, _fl, _eh, _phes, _phn, e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHIIIIIHHHHHH", data, 16)
	if e_machine != 3:
		raise PackError("not an Intel 80386 ELF file")
	secs = []
	for i in range(e_shnum):
		name, typ, flags, addr, off, size, link, info, _al, _es = struct.unpack_from("<IIIIIIIIII", data, e_shoff + i * e_shentsize)
		secs.append({"name_off": name, "type": typ, "flags": flags, "addr": addr, "off": off, "size": size, "link": link})
	shstr = secs[e_shstrndx]
	for s in secs:
		end = data.index(b"\0", shstr["off"] + s["name_off"])
		s["name"] = data[shstr["off"] + s["name_off"] : end].decode()
	by_name = {s["name"]: s for s in secs}
	syms = {}
	symtab = by_name.get(".symtab")
	if symtab:
		strtab = secs[symtab["link"]]
		for i in range(symtab["size"] // 16):
			st_name, st_value, _sz, _info, _other, st_shndx = struct.unpack_from("<IIIBBH", data, symtab["off"] + 16 * i)
			end = data.index(b"\0", strtab["off"] + st_name)
			nm = data[strtab["off"] + st_name : end].decode()
			if nm:
				syms[nm] = (st_value, secs[st_shndx]["name"] if st_shndx < len(secs) else "")
	return secs, by_name, syms


def section_bytes(data, sec):
	if sec is None or sec["type"] == 8:  # NOBITS
		return b"" if sec is None else bytes(sec["size"])
	return data[sec["off"] : sec["off"] + sec["size"]]


def dialog_template(d):
	"""Encodes one dialog description as a Win16 DLGTEMPLATE (words for the geometry, a class byte of 0x80 or more for stock controls)."""
	style = d.get("style")
	if style is None:
		style = WS["popup"] | WS["caption"] | WS["sysmenu"] | WS["modalframe"] | WS["setfont"]
	elif isinstance(style, list):
		style = sum(WS[s] if isinstance(s, str) else s for s in style)
	items = d.get("items", [])
	out = bytearray()
	out += struct.pack("<IB", style, len(items))
	out += struct.pack("<hhhh", d["x"], d["y"], d["cx"], d["cy"])
	out += b"\0"  # no menu
	out += b"\0"  # default class
	out += d.get("caption", "").encode("latin-1") + b"\0"
	if style & WS["setfont"]:
		out += struct.pack("<H", d.get("font_size", 8)) + d.get("font", "MS Sans Serif").encode("latin-1") + b"\0"
	for it in items:
		istyle = it.get("style", [])
		if isinstance(istyle, list):
			istyle = sum(WS[s] if isinstance(s, str) else s for s in istyle)
		out += struct.pack("<hhhhH", it["x"], it["y"], it["cx"], it["cy"], it["id"])
		out += struct.pack("<I", istyle | WS["child"] | WS["visible"])
		cls = it["class"]
		out += bytes([CLASS_IDS[cls]]) if cls in CLASS_IDS else cls.encode("latin-1") + b"\0"
		out += it.get("text", "").encode("latin-1") + b"\0"
		out += b"\0"  # no creation data
	return bytes(out)


def string_blocks(strings):
	"""Groups string IDs into the 16-string blocks an RT_STRING resource holds. Returns {block_id: bytes}."""
	blocks = {}
	for sid, text in strings.items():
		sid = int(sid)
		blocks.setdefault(sid // 16 + 1, {})[sid % 16] = text.encode("latin-1")
	res = {}
	for bid, items in blocks.items():
		b = bytearray()
		for i in range(16):
			s = items.get(i, b"")
			if len(s) > 255:
				raise PackError("string too long")
			b += bytes([len(s)]) + s
		res[bid] = bytes(b)
	return res


def load_resources(path):
	"""Returns a sorted list of (type_id, res_id, bytes) from a JSON description."""
	if not path:
		return []
	base = os.path.dirname(os.path.abspath(path))
	with open(path, encoding="utf-8") as f:
		spec = json.load(f)
	res = []
	for d in spec.get("dialogs", []):
		res.append((RT_DIALOG, d["id"], dialog_template(d)))
	for bid, blob in string_blocks(spec.get("strings", {})).items():
		res.append((RT_STRING, bid, blob))
	for r in spec.get("rcdata", []):
		if "file" in r:
			with open(os.path.join(base, r["file"]), "rb") as f:
				blob = f.read()
		else:
			blob = r["text"].encode("latin-1")
		# NE resource lengths are in units of the alignment, so the exact size is lost; keep it in a 4-byte prefix.
		blob = struct.pack("<I", len(blob)) + blob
		res.append((RT_RCDATA, r["id"], blob))
	return sorted(res, key=lambda r: (r[0], r[1]))


def resource_table(resources, data_base):
	"""Builds the resource table. `data_base` is where the resource data starts in the file. Returns (table bytes, data bytes)."""
	types = {}
	for t, i, blob in resources:
		types.setdefault(t, []).append((i, blob))
	pos = data_base
	body = bytearray()
	data = bytearray()
	body += struct.pack("<H", RES_SHIFT)
	for t in sorted(types):
		items = types[t]
		body += struct.pack("<HHI", 0x8000 | t, len(items), 0)
		for i, blob in items:
			off = pos + len(data)
			if len(blob) > 0xFFFF << RES_SHIFT:
				raise PackError("resource too large")
			# Both the offset and the length are in units of the resource alignment (1 << RES_SHIFT), so every resource is padded to one.
			units = (len(blob) + (1 << RES_SHIFT) - 1) >> RES_SHIFT
			body += struct.pack("<HHHHHH", off >> RES_SHIFT, units, RES_FLAGS, 0x8000 | i, 0, 0)
			data += blob + bytes((units << RES_SHIFT) - len(blob))
	body += struct.pack("<H", 0)  # end of types
	body += b"\0"  # end of resource names
	return bytes(body), bytes(data)


def pack(elf_data, module="SETUP", description="", stack=0x2000, heap=0x1000, resources=(), expected_ver=0x030A):
	secs, by_name, syms = read_elf(elf_data)
	code = section_bytes(elf_data, by_name.get(".text"))
	dat = section_bytes(elf_data, by_name.get(".data"))
	bss = by_name.get(".bss")
	if bss:
		dat += bytes(bss["size"])
	if not code:
		raise PackError("no .text")
	if len(code) > 0xFFFF:
		raise PackError(f"code is {len(code)} bytes, over one 64 KB segment")
	if len(dat) < 16:
		dat += bytes(16 - len(dat))
	if len(dat) + stack + heap > 0x10000:
		raise PackError(f"data {len(dat)} + stack {stack} + heap {heap} is over one 64 KB segment")
	if "_start" not in syms:
		raise PackError("no _start")

	imports = []  # (module, ordinal, offset)
	exports = []  # (name, offset)
	selfix = []  # (offset, segment)
	for nm, (val, sec) in syms.items():
		m = re.fullmatch(r"__w16imp_([A-Z0-9]+)_(\d+)_\w+", nm)
		if m:
			imports.append((m.group(1), int(m.group(2)), val))
			continue
		m = re.fullmatch(r"__w16exp_(\w+)", nm)
		if m:
			exports.append((m.group(1), val))
			continue
		m = re.fullmatch(r"__w16sel_(code|data)_\d+", nm)
		if m:
			selfix.append((val, 1 if m.group(1) == "code" else 2))
	exports.sort()
	imports.sort(key=lambda r: r[2])
	mod_names = []
	for mn, _o, _v in imports:
		if mn not in mod_names:
			mod_names.append(mn)
	mod_names.sort(key=lambda n: ("KERNEL", "USER", "GDI").index(n) if n in ("KERNEL", "USER", "GDI") else 99)

	# Relocation records for segment 1: 8 bytes each. A fixup that is not additive is a link in a chain: the loader reads the offset of
	# the next fixup from the location it is about to patch, and 0xFFFF ends the chain. Each site here is a chain of one.
	code = bytearray(code)
	for _m, _o, val in imports:
		struct.pack_into("<H", code, val, 0xFFFF)
	for val, _seg in selfix:
		struct.pack_into("<H", code, val, 0xFFFF)
	code = bytes(code)
	relocs = []
	for mn, o, val in imports:
		relocs.append((val, struct.pack("<BBHHH", 3, 1, val, mod_names.index(mn) + 1, o)))
	for val, seg in selfix:
		relocs.append((val, struct.pack("<BBHBBH", 2, 0, val, seg, 0, 0)))
	relocs.sort(key=lambda r: r[0])
	reloc_blob = struct.pack("<H", len(relocs)) + b"".join(r[1] for r in relocs)

	# Name tables.
	def name_entry(name, ordinal):
		b = name.encode("latin-1")
		return bytes([len(b)]) + b + struct.pack("<H", ordinal)

	resident = name_entry(module, 0) + b"".join(name_entry(n.upper(), i + 1) for i, (n, _o) in enumerate(exports)) + b"\0"
	nonres = (name_entry(description, 0) + b"\0") if description else b"\0"

	imp_names = bytearray(b"\0")
	modref = bytearray()
	for n in mod_names:
		modref += struct.pack("<H", len(imp_names))
		imp_names += bytes([len(n)]) + n.encode()

	entry = bytearray()
	if exports:
		entry += bytes([len(exports), 1])  # a bundle of fixed entries in segment 1
		for _n, off in exports:
			entry += struct.pack("<BH", 0x03, off)
	entry += b"\0\0"

	# Layout. The MZ stub is 0x80 bytes, the NE header 0x40, then the tables, each directly after the last.
	ne_off = 0x80
	seg_tab = ne_off + 0x40
	res_tab = seg_tab + 16
	res_blob_dummy, _ = resource_table(resources, 0) if resources else (b"", b"")
	res_len = len(res_blob_dummy)
	resname_tab = res_tab + res_len
	modref_tab = resname_tab + len(resident)
	impname_tab = modref_tab + len(modref)
	entry_tab = impname_tab + len(imp_names)
	nonres_tab = entry_tab + len(entry)
	tables_end = nonres_tab + len(nonres)
	code_off = align(tables_end, 1 << FILE_SHIFT)
	data_off = align(code_off + len(code) + len(reloc_blob), 1 << FILE_SHIFT)
	res_data_base = align(data_off + len(dat), 1 << RES_SHIFT)
	res_blob, res_data = resource_table(resources, res_data_base) if resources else (b"", b"")

	seg_code = struct.pack("<HHHH", code_off >> FILE_SHIFT, len(code), SEG_PRELOAD | SEG_RELOCS | SEG_RING3, len(code))
	seg_data = struct.pack("<HHHH", data_off >> FILE_SHIFT, len(dat), SEG_DATA | SEG_PRELOAD | SEG_RING3, len(dat))

	hdr = bytearray(0x40)
	hdr[0:2] = b"NE"
	hdr[2] = 5
	hdr[3] = 1
	struct.pack_into("<HH", hdr, 4, entry_tab - ne_off, len(entry))
	struct.pack_into("<I", hdr, 8, 0)
	struct.pack_into("<HHHH", hdr, 0x0C, NE_WINAPI | NE_MULTIPLEDATA, 2, heap, stack)
	struct.pack_into("<HHHH", hdr, 0x14, syms["_start"][0], 1, 0, 2)
	struct.pack_into("<HHHH", hdr, 0x1C, 2, len(mod_names), len(nonres), seg_tab - ne_off)
	struct.pack_into("<HHHH", hdr, 0x24, res_tab - ne_off, resname_tab - ne_off, modref_tab - ne_off, impname_tab - ne_off)
	struct.pack_into("<I", hdr, 0x2C, nonres_tab)
	struct.pack_into("<HH", hdr, 0x30, 0, FILE_SHIFT)
	struct.pack_into("<H", hdr, 0x34, sum(1 for _ in resources))
	hdr[0x36] = 2  # Windows
	struct.pack_into("<H", hdr, 0x3E, expected_ver)
	if not resources:
		struct.pack_into("<H", hdr, 0x24, resname_tab - ne_off)
	# Entry offsets are measured from the NE header, as are all table offsets above.

	# The DOS header: a one-page image, a 4-paragraph header, the relocation table at 0x40 and the NE header's offset at 0x3C.
	mz = bytearray(0x80)
	struct.pack_into("<2s13H", mz, 0, b"MZ", 0x80, 1, 0, 4, 0, 0xFFFF, 0, 0xB8, 0, 0, 0, 0x40, 0)
	struct.pack_into("<I", mz, 0x3C, ne_off)
	stub = bytes([0x0E, 0x1F, 0xBA, 0x0E, 0x00, 0xB4, 0x09, 0xCD, 0x21, 0xB8, 0x01, 0x4C, 0xCD, 0x21]) + MZ_STUB_MESSAGE
	mz[0x40 : 0x40 + len(stub)] = stub

	out = bytearray()
	out += mz
	out += hdr
	out += seg_code + seg_data
	out += res_blob
	out += resident + modref + imp_names + entry + nonres
	out += bytes(code_off - len(out))
	out += code + reloc_blob
	out += bytes(data_off - len(out))
	out += dat
	if resources:
		out += bytes(res_data_base - len(out))
		out += res_data
	info = {
		"code": len(code),
		"data": len(dat),
		"stack": stack,
		"heap": heap,
		"imports": len(imports),
		"exports": [n for n, _o in exports],
		"modules": mod_names,
		"relocs": len(relocs),
		"resources": len(resources),
		"file": len(out),
	}
	return bytes(out), info


def main():
	args = sys.argv[1:]
	opts = {"module": "SETUP", "description": "", "stack": 0x2000, "heap": 0x1000}
	res_path = None
	while args and args[0].startswith("--"):
		o = args.pop(0)
		if o == "--module":
			opts["module"] = args.pop(0)
		elif o == "--description":
			opts["description"] = args.pop(0)
		elif o in ("--stack", "--heap"):
			opts[o[2:]] = int(args.pop(0), 0)
		elif o == "--resources":
			res_path = args.pop(0)
		else:
			sys.exit("ne_pack: unknown option " + o)
	if len(args) != 2:
		sys.exit(__doc__)
	try:
		with open(args[0], "rb") as f:
			out, info = pack(f.read(), resources=load_resources(res_path), **opts)
	except PackError as e:
		sys.exit("ne_pack: " + str(e))
	with open(args[1], "wb") as f:
		f.write(out)
	print(f"ne_pack: {args[1]}: {info['file']} bytes (code {info['code']}, data {info['data']} + stack {info['stack']} + heap {info['heap']}, {info['imports']} imports, {info['resources']} resources)")


if __name__ == "__main__":
	main()
