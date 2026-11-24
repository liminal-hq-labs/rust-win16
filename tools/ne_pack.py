#!/usr/bin/env python3
# Packs a linked 32-bit x86 ELF built for the win16 target into a Win16 NE executable that Windows 3.x, 9x and Wine's krnl386 load.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# The linker resolves every symbol; this tool places the code in segments and writes the tables Windows reads. link.ld puts the data at
# address 0 and, with `--unique` and `--emit-relocs`, leaves every code section an output section of its own with its relocations. Each
# code section is a placement unit: the packer puts units into code segments (1, then 3 and up; 2 is the automatic data segment),
# recomputes every relocation that points into code, and sends a branch that crosses segments through a veneer that makes a far jump
# with an NE fixup. docs/packer.md explains the veneers, the shadow return stack and the slot table that function pointers use.
#
# An ELF without relocations (the old single-overlay link.ld) is one unit, packed as one code segment exactly as linked.
#
# Symbols the packer reads from the ELF symbol table:
#   __w16imp_<MODULE>_<ordinal>_<Name>   a 4-byte far-call operand in code: an imported-ordinal fixup (type "far address") is written for it
#   __w16exp_<Name>               the start of an exported far procedure (segment 1): an entry-table entry and a resident name
#   __w16sel_code_<n>             a 16-bit word in code that must hold segment 1's selector
#   __w16sel_data_<n>             a 16-bit word in code that must hold the data segment's selector
#   __w16sel_seg<N>_<n>           a 16-bit word in code that must hold code segment N's selector
#   w16_shadow_overflow           optional: where a full shadow return stack sends the program (the runtime defines it)
#   _start                        the entry point (CS:IP in the header)
#
# Resources come from a JSON file (see scripts/build.sh and examples/*/resources.json): dialogs, strings and raw data, each with an
# integer ID. Raw data (RT_RCDATA) starts with its exact length as a little-endian 32-bit number. Segments come from a JSON file too
# (`--segments`, see docs/packer.md).
import fnmatch
import json
import os
import re
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rust_demangle import demangle  # noqa: E402

USAGE = """usage: ne_pack.py [--module NAME] [--description TEXT] [--stack N] [--heap N] [--resources FILE.json]
                  [--segments FILE.json] [--shadow N] [--map FILE] <in.elf> <out.exe>"""

MZ_STUB_MESSAGE = b"This program requires Microsoft Windows.\r\n$"
SEG_DATA = 0x0001
SEG_MOVEABLE = 0x0010
SEG_PRELOAD = 0x0040
SEG_RELOCS = 0x0100
SEG_RING3 = 0x0C00
SEG_DISCARDABLE = 0x1000
NE_MULTIPLEDATA = 0x0002
NE_WINAPI = 0x0300
RES_FLAGS = 0x1030  # MOVEABLE | PURE | DISCARDABLE
RES_SHIFT = 4
FILE_SHIFT = 4
RT_DIALOG = 5
RT_STRING = 6
RT_RCDATA = 10
DATA_SEG = 2
MAX_SEG = 0xFFFF  # the longest segment the packer writes; a length of 0 would mean 64 KB, which nothing here needs
DEFAULT_BUDGET = 0xF000  # bytes of units per automatically filled segment, leaving room for veneers and the slot table
DEFAULT_SHADOW = 256  # shadow return stack entries, 4 bytes each
SLOT_SIZE = 10
RELOC_SELECTOR = 2
RELOC_FAR = 3
R_386_32 = 1
R_386_PC32 = 2
R_386_PLT32 = 4  # in a static link, the same as R_386_PC32
R_386_16 = 20
R_386_PC16 = 21
R_386_8 = 22
R_386_PC8 = 23
RELOC_SIZES = {R_386_32: 4, R_386_PC32: 4, R_386_PLT32: 4, R_386_16: 2, R_386_PC16: 2, R_386_8: 1, R_386_PC8: 1}
RELOC_NAMES = {R_386_32: "R_386_32", R_386_PC32: "R_386_PC32", R_386_PLT32: "R_386_PLT32", R_386_16: "R_386_16", R_386_PC16: "R_386_PC16", R_386_8: "R_386_8", R_386_PC8: "R_386_PC8"}
SHF_ALLOC = 2
SHF_EXECINSTR = 4
SHT_SYMTAB = 2
SHT_RELA = 4
SHT_NOBITS = 8
SHT_REL = 9
STT_FUNC = 2
SHN_UNDEF = 0
SHN_ABS = 0xFFF1
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


def p16(v):
	return struct.pack("<H", v & 0xFFFF)


def read_elf(data):
	"""Returns (sections, symbols, relocations by the index of the section they patch)."""
	if data[:4] != b"\x7fELF" or data[4] != 1 or data[5] != 1:
		raise PackError("not a 32-bit little-endian ELF file")
	_t, e_machine, _v, _entry, _phoff, e_shoff, _fl, _eh, _phes, _phn, e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHIIIIIHHHHHH", data, 16)
	if e_machine != 3:
		raise PackError("not an Intel 80386 ELF file")
	secs = []
	for i in range(e_shnum):
		name, typ, flags, addr, off, size, link, info, al, _es = struct.unpack_from("<IIIIIIIIII", data, e_shoff + i * e_shentsize)
		secs.append({"index": i, "name_off": name, "type": typ, "flags": flags, "addr": addr, "off": off, "size": size, "link": link, "info": info, "align": max(al, 1)})
	shstr = secs[e_shstrndx]
	for s in secs:
		end = data.index(b"\0", shstr["off"] + s["name_off"])
		s["name"] = data[shstr["off"] + s["name_off"] : end].decode()
	syms = []
	symtab = next((s for s in secs if s["type"] == SHT_SYMTAB), None)
	if symtab:
		strtab = secs[symtab["link"]]
		for i in range(symtab["size"] // 16):
			st_name, st_value, st_size, st_info, _other, st_shndx = struct.unpack_from("<IIIBBH", data, symtab["off"] + 16 * i)
			end = data.index(b"\0", strtab["off"] + st_name)
			syms.append({"name": data[strtab["off"] + st_name : end].decode(), "value": st_value, "size": st_size, "type": st_info & 15, "shndx": st_shndx})
	rels = {}
	for s in secs:
		if s["type"] == SHT_RELA:
			raise PackError(f"{s['name']}: RELA relocations are not expected for i386")
		if s["type"] == SHT_REL:
			lst = rels.setdefault(s["info"], [])
			for k in range(s["size"] // 8):
				off, info = struct.unpack_from("<II", data, s["off"] + 8 * k)
				lst.append((off, info & 0xFF, info >> 8))
	return secs, syms, rels


def section_bytes(data, sec):
	if sec is None or sec["type"] == SHT_NOBITS:
		return b"" if sec is None else bytes(sec["size"])
	return data[sec["off"] : sec["off"] + sec["size"]]


class Unit:
	"""One code section: the unit the packer places. `base` is its offset in segment `seg` once placed."""

	def __init__(self, sec, data):
		self.index = sec["index"]
		self.name = sec["name"]
		self.addr = sec["addr"]
		self.size = sec["size"]
		self.align = sec["align"]
		self.code = bytearray(section_bytes(data, sec))
		rest = self.name[len(".text.") :] if self.name.startswith(".text.") else self.name
		rest = re.sub(r"^(unlikely|hot|startup|exit)\.", "", rest)
		self.path = demangle(rest)
		self.names = [self.name, self.path]
		self.func_starts = {0}
		self.exports = set()
		self.seg = None
		self.base = None

	def label(self):
		return self.path if self.path != self.name else self.name


class Segment:
	def __init__(self, number, conf, auto):
		self.number = number
		self.name = conf.get("name", "")
		self.budget = conf.get("budget", DEFAULT_BUDGET)
		self.preload = conf.get("preload", True)
		self.discardable = conf.get("discardable", False)
		self.auto = auto
		self.units = []

	def flags(self):
		if self.number == 1:
			return SEG_PRELOAD | SEG_RELOCS | SEG_RING3  # fixed: the entry point, the exported procedures and the import stubs live here
		f = SEG_MOVEABLE | SEG_RELOCS | SEG_RING3
		if self.preload:
			f |= SEG_PRELOAD
		if self.discardable:
			f |= SEG_DISCARDABLE
		return f


SEGMENT_KEYS = {"name", "place", "budget", "preload", "discardable"}


def check_config(cfg):
	"""Checks a segments description (see docs/packer.md) and returns it with the segment numbers as integers."""
	if not isinstance(cfg, dict) or set(cfg) - {"segments", "auto", "shadow"}:
		raise PackError("segments: the top level takes only `segments`, `auto` and `shadow`")
	segs = {}
	for k, v in cfg.get("segments", {}).items():
		if not str(k).isdigit() or int(k) in (0, DATA_SEG) or int(k) > 254:
			raise PackError(f"segments: {k!r} is not a code segment number (1, or 3 to 254; 2 is the data segment)")
		if not isinstance(v, dict) or set(v) - SEGMENT_KEYS:
			raise PackError(f"segments: segment {k} takes only {', '.join(sorted(SEGMENT_KEYS))}")
		if int(k) == 1 and ("preload" in v or "discardable" in v):
			raise PackError("segments: segment 1 is always fixed and preloaded")
		segs[int(k)] = v
	auto = cfg.get("auto", {})
	if not isinstance(auto, dict) or set(auto) - {"budget", "preload", "discardable"}:
		raise PackError("segments: `auto` takes only budget, preload and discardable")
	for c in list(segs.values()) + [auto]:
		b = c.get("budget", DEFAULT_BUDGET)
		if not isinstance(b, int) or not 0 < b <= MAX_SEG:
			raise PackError(f"segments: a budget of {b!r} is not between 1 and {MAX_SEG}")
	return {"segments": segs, "auto": auto, "shadow": cfg.get("shadow", DEFAULT_SHADOW)}


def pinned(name):
	"""Sections that must be in segment 1: the entry code, the exported procedures, the import stubs and the runtime's fixed helpers."""
	return name == ".text.start" or name.startswith((".text.exp_", ".text.imp_")) or name == ".text.w16fixed" or name.startswith(".text.w16fixed.")


def assign(units, cfg, caps):
	"""Puts every unit in a segment. Returns ({number: Segment}, multi). `caps` overrides the budgets of automatically filled segments."""
	conf = cfg["segments"]
	segs = {}

	def seg(n, auto=False):
		if n not in segs:
			segs[n] = Segment(n, conf.get(n, cfg["auto"] if auto else {}), auto or n == 1)
			if n in caps:
				segs[n].budget = caps[n]
		return segs[n]

	seg(1)
	for n in conf:
		seg(n)
	rest = []
	for u in units:
		n = None
		m = re.fullmatch(r"\.text\.seg(\d+)(?:\..*)?", u.name)
		if pinned(u.name):
			n = 1
		elif m:
			n = int(m.group(1))
			if n in (0, DATA_SEG) or n > 254:
				raise PackError(f"{u.name}: {n} is not a code segment number (1, or 3 to 254)")
		else:
			for cn in sorted(conf):
				if any(fnmatch.fnmatchcase(x, pat) or fnmatch.fnmatchcase(x.lstrip("<"), pat) for pat in conf[cn].get("place", []) for x in u.names):
					n = cn
					break
		if n is None:
			rest.append(u)
		else:
			seg(n).units.append(u)
	total = sum(u.size + u.align - 1 for u in units)
	if len(segs) == 1 and total <= conf.get(1, {}).get("budget", MAX_SEG) and not caps:
		segs[1].units += rest
		return segs, False
	cur = segs[1]
	used = sum(u.size for u in cur.units)
	for u in rest:
		if used and used + u.size > cur.budget:
			n = 3
			while n in segs:
				n += 1
			cur = seg(n, auto=True)
			used = 0
		cur.units.append(u)
		used += u.size
	for n, sg in segs.items():
		if n != 1 and not sg.auto and "budget" in conf.get(n, {}):
			placed = sum(u.size for u in sg.units)
			if placed > sg.budget:
				raise PackError(f"code segment {n} holds {placed} bytes of code, over its budget of {sg.budget}")
	numbers = sorted(segs)
	missing = [n for n in range(3, numbers[-1] + 1) if n not in segs or not segs[n].units]
	if missing:
		raise PackError(f"code segments must be numbered without gaps; segment {', '.join(map(str, missing))} has no code")
	return segs, len(segs) > 1


def branch_kind(code, off, size):
	"""Names the instruction whose relative operand of `size` bytes starts at `off`: call, jump or None."""
	op = code[off - 1] if off >= 1 else None
	op2 = code[off - 2] if off >= 2 else None
	if size == 4:
		if op == 0xE8 and op2 == 0x66:
			return "call"
		if op == 0xE9 and op2 == 0x66:
			return "jump"
		if op2 == 0x0F and op is not None and 0x80 <= op <= 0x8F:
			return "jump"
	if size == 2:
		if op == 0xE9:
			return "jump"
		if op2 == 0x0F and op is not None and 0x80 <= op <= 0x8F:
			return "jump"
		if op == 0xE8:
			return "call16"
	return None


def shared_header(ssp, limit, ovf_far, gated):
	"""The code at the start of every segment of a multi-segment program. Returns (bytes, tramp offset, dyn offset, far jump sites), the
	sites as (offset, "overflow" or "gate"). With `gated`, the trampoline and the dynamic veneer leave through the gate in segment 1
	instead of returning far themselves, since their target may be a segment that is not in memory."""
	b = bytearray(b"\xcc" * 4)  # offset 0 never holds a function, so a null function pointer cannot call one
	tramp = len(b)
	# The return trampoline, at the same offset in every segment. A callee entered through the dynamic veneer returns here with a near
	# return; the trampoline pops the caller's CS:IP from the shadow stack and returns far to it. No general register is changed: EAX,
	# ECX and EDX may hold results, and the rest belong to the caller.
	b += bytes([0x83, 0xEC, 0x04])  # sub sp, 4 (room for the far return address)
	b += bytes([0x55, 0x89, 0xE5, 0x53])  # push bp; mov bp, sp; push bx
	b += bytes([0x36, 0x8B, 0x1E]) + p16(ssp)  # mov bx, ss:[ssp]
	b += bytes([0x83, 0xEB, 0x04])  # sub bx, 4
	b += bytes([0x36, 0x89, 0x1E]) + p16(ssp)  # mov ss:[ssp], bx
	b += bytes([0x36, 0xFF, 0x37, 0x8F, 0x46, 0x02])  # push word ptr ss:[bx]; pop word ptr [bp+2]
	b += bytes([0x36, 0xFF, 0x77, 0x02, 0x8F, 0x46, 0x04])  # push word ptr ss:[bx+2]; pop word ptr [bp+4]
	b += bytes([0x5B, 0x5D])  # pop bx; pop bp
	sites = []

	def leave():
		if gated:
			sites.append((len(b) + 1, "gate"))
			b.extend(b"\xea\xff\xff\x00\x00")  # jmp far gate, which returns far to [sp]
		else:
			b.append(0xCB)  # retf

	leave()
	dyn = len(b)
	# The dynamic veneer, entered with [sp] = target offset, [sp+2] = target selector, [sp+4] = the 4-byte return address. It saves the
	# caller's CS and return offset on the shadow stack, points the return address at the trampoline and returns far into the target.
	# When the return address already is the trampoline (a tail call made from code that was itself entered this way), the entry on
	# the shadow stack is still the right one, so a chain of tail calls across segments uses one entry. Only the low halves of BP, BX
	# and AX are used, and they are restored.
	b += bytes([0x55, 0x89, 0xE5, 0x53, 0x50])  # push bp; mov bp, sp; push bx; push ax
	b += bytes([0x81, 0x7E, 0x06]) + p16(tramp)  # cmp word ptr [bp+6], tramp
	jne = len(b)
	b += bytes([0x75, 0x00])  # jne save
	b += bytes([0x83, 0x7E, 0x08, 0x00])  # cmp word ptr [bp+8], 0
	je = len(b)
	b += bytes([0x74, 0x00])  # je done
	b[jne + 1] = len(b) - (jne + 2)
	b += bytes([0x36, 0x8B, 0x1E]) + p16(ssp)  # save: mov bx, ss:[ssp]
	b += bytes([0x81, 0xFB]) + p16(limit)  # cmp bx, limit
	jae = len(b)
	b += bytes([0x73, 0x00])  # jae overflow
	b += bytes([0x8B, 0x46, 0x06])  # mov ax, [bp+6]
	b += bytes([0x36, 0x89, 0x07])  # mov ss:[bx], ax
	b += bytes([0x36, 0x8C, 0x4F, 0x02])  # mov ss:[bx+2], cs
	b += bytes([0x83, 0xC3, 0x04])  # add bx, 4
	b += bytes([0x36, 0x89, 0x1E]) + p16(ssp)  # mov ss:[ssp], bx
	b += bytes([0xC7, 0x46, 0x06]) + p16(tramp)  # mov word ptr [bp+6], tramp
	b[je + 1] = len(b) - (je + 2)
	b += bytes([0x58, 0x5B, 0x5D])  # done: pop ax; pop bx; pop bp
	leave()
	ovf = len(b)
	b[jae + 1] = ovf - (jae + 2)
	if ovf_far:
		b += bytes([0xEA, 0xFF, 0xFF, 0x00, 0x00])  # jmp far w16_shadow_overflow
		sites.append((ovf + 1, "overflow"))
	else:
		b += bytes([0xB8, 0x7F, 0x4C, 0xCD, 0x21])  # mov ax, 4C7Fh; int 21h
	while len(b) % 2:
		b += b"\xcc"
	return bytes(b), tramp, dyn, sites


def gate_code(ss16, ss32, sync_rel):
	"""The gate in segment 1, entered by a far jump with [sp] = target offset and [sp+2] = target selector. The target segment may be
	load-on-call or discarded: loading its selector into ES on the 16-bit stack, with every register saved, lets Windows take the
	not-present fault and load it as it would for any 16-bit program. Loading can move the data segment, so the stack alias is synced
	(`sync_rel(at)` gives the call's displacement). Then the gate returns far into the target."""
	b = bytearray()
	b += bytes([0x66, 0x60, 0x06, 0x89, 0xE5])  # pushad; push es; mov bp, sp
	b += bytes([0x8B, 0x46, 0x24])  # mov ax, [bp+36] (the selector)
	b += bytes([0x8E, 0x16]) + p16(ss16) + bytes([0x66, 0x0F, 0xB7, 0xE4])  # mov ss, [__w16_ss16]; movzx esp, sp
	b += bytes([0x8E, 0xC0])  # mov es, ax
	b += bytes([0x66, 0xE8]) + struct.pack("<i", sync_rel(len(b) + 6))  # call w16_sync_alias
	b += bytes([0x8E, 0x16]) + p16(ss32) + bytes([0x66, 0x0F, 0xB7, 0xE4])  # mov ss, [__w16_ss32]; movzx esp, sp
	b += bytes([0x07, 0x66, 0x61, 0xCB])  # pop es; popad; retf
	return bytes(b)


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



class Ref:
	"""A relocation that points into code. `where` is "code" (in `unit` at `off`) or "data" (at `off` in the data segment)."""

	def __init__(self, where, unit, off, typ, target, ref, sym):
		self.where = where
		self.unit = unit
		self.off = off
		self.typ = typ
		self.size = RELOC_SIZES[typ]
		self.pc = typ in (R_386_PC32, R_386_PLT32, R_386_PC16, R_386_PC8)
		self.target = target
		self.ref = ref
		self.sym = sym
		self.kind = None

	def site(self):
		if self.where == "data":
			return f"data offset {self.off:#x}"
		return f"{self.unit.label()}+{self.off:#x}"


def read_refs(elf_data, secs, syms, rels, units, data_secs):
	"""Turns every relocation into a Ref when it points into code. Relocations that point at data are already final and are left."""
	by_index = {u.index: u for u in units}
	refs = []
	for target_index, lst in sorted(rels.items()):
		sec = secs[target_index]
		if target_index in by_index:
			where, unit = "code", by_index[target_index]
		elif target_index in data_secs:
			where, unit = "data", None
		elif not sec["flags"] & SHF_ALLOC:
			continue
		else:
			raise PackError(f"relocations in {sec['name']}, which is neither code nor the data segment")
		for addr, typ, symi in lst:
			if typ == 0:
				continue
			if typ not in RELOC_SIZES:
				raise PackError(f"relocation type {typ} at {addr:#x} in {sec['name']} is not supported")
			sym = syms[symi]
			name = demangle(sym["name"]) if sym["name"] else "(section)"
			if sym["shndx"] == SHN_UNDEF:
				raise PackError(f"undefined symbol {name}")
			size = RELOC_SIZES[typ]
			pc = typ in (R_386_PC32, R_386_PLT32, R_386_PC16, R_386_PC8)
			tu = by_index.get(sym["shndx"])
			if tu is None:
				if pc and where == "code" and sym["shndx"] != SHN_ABS:
					raise PackError(f"a relative reference to data ({name}) in {sec['name']}")
				continue
			if where == "code":
				off = addr - unit.addr
				field = unit.code[off : off + size]
			else:
				start, sec_addr = data_secs[target_index]
				off = start + addr - sec_addr
				field = section_bytes(elf_data, sec)[addr - sec_addr : addr - sec_addr + size]
			v = int.from_bytes(field, "little")
			bits = 8 * size
			a = (v - sym["value"] + (addr if pc else 0)) & ((1 << bits) - 1)
			if a >= 1 << (bits - 1):
				a -= 1 << bits
			ref = sym["value"] - tu.addr + a + (size if pc else 0)
			if not 0 <= ref <= tu.size:
				raise PackError(f"{RELOC_NAMES[typ]} at {addr:#x} in {sec['name']} points outside {tu.label()}")
			refs.append(Ref(where, unit, off, typ, tu, ref, name if sym["type"] != 3 else tu.label()))
	return refs


def write_field(buf, off, size, value, signed, what):
	bits = 8 * size
	if size == 2 and signed:
		value &= 0xFFFF  # 16-bit IP arithmetic wraps within the segment
	lo, hi = (-(1 << (bits - 1)), (1 << (bits - 1)) - 1) if signed else (0, (1 << bits) - 1)
	if not lo <= value <= hi:
		if not (signed and size == 2):
			raise PackError(f"{what}: {value:#x} does not fit in {bits} bits")
	buf[off : off + size] = (value & ((1 << bits) - 1)).to_bytes(size, "little")


def pack(elf_data, module="SETUP", description="", stack=0x2000, heap=0x1000, resources=(), expected_ver=0x030A, segments=None, shadow=None):
	secs, syms, rels = read_elf(elf_data)
	cfg = check_config(segments or {})
	if shadow is None:
		shadow = cfg["shadow"]
	if not isinstance(shadow, int) or not 16 <= shadow <= 4096:
		raise PackError(f"a shadow stack of {shadow!r} entries is not between 16 and 4096")

	units = [Unit(s, elf_data) for s in secs if s["flags"] & SHF_ALLOC and s["flags"] & SHF_EXECINSTR and s["size"]]
	if not units:
		raise PackError("no code: the ELF has no executable section")
	units.sort(key=lambda u: (u.name != ".text.start", u.addr, u.index))
	by_index = {u.index: u for u in units}

	# The data segment: `.data`, then `.bss` when the linker kept it apart. Code reaches data through offsets the linker already wrote.
	data_secs = {}
	dat = bytearray()
	for s in secs:
		if not s["flags"] & SHF_ALLOC or s["flags"] & SHF_EXECINSTR or not s["size"]:
			continue
		if s["name"] not in (".data", ".bss"):
			raise PackError(f"section {s['name']} is neither code nor data; link.ld folds read-only data and zero data into .data")
		data_secs[s["index"]] = (len(dat), s["addr"])
		if s["name"] == ".data" and s["addr"] != 0 and rels:
			raise PackError(".data must be linked at address 0, the start of the data segment")
		dat += section_bytes(elf_data, s)
	if len(dat) < 16:
		dat += bytes(16 - len(dat))

	for s in syms:
		u = by_index.get(s["shndx"])
		if u is None:
			continue
		if s["type"] == STT_FUNC and s["value"] - u.addr:
			u.func_starts.add(s["value"] - u.addr)
			u.names.append(demangle(s["name"]))
		if s["name"].startswith("__w16exp_"):
			u.exports.add(s["value"] - u.addr)

	def code_loc(name):
		s = next((s for s in syms if s["name"] == name), None)
		if s is None:
			return None
		u = by_index.get(s["shndx"])
		if u is None:
			raise PackError(f"{name} is not in code")
		return u, s["value"] - u.addr

	if code_loc("_start") is None:
		raise PackError("no _start")
	for u in units:
		if u.size > MAX_SEG:
			raise PackError(f"code section {u.label()} is {u.size} bytes, over one 64 KB segment")

	refs = read_refs(elf_data, secs, syms, rels, units, data_secs)
	ovf = code_loc("w16_shadow_overflow")
	gate_syms = None
	data_syms = {s["name"]: s for s in syms if s["shndx"] in data_secs}
	if "__w16_ss16" in data_syms and "__w16_ss32" in data_syms and code_loc("w16_sync_alias"):
		gate_syms = {"ss16": data_syms["__w16_ss16"]["value"], "ss32": data_syms["__w16_ss32"]["value"], "sync": code_loc("w16_sync_alias")}

	# Place, lay out and, if a segment comes out too long once its veneers are added, lower that segment's budget and place again.
	caps = {}
	for _attempt in range(12):
		segs, multi = assign(units, cfg, caps)
		for n, sg in segs.items():
			for u in sg.units:
				u.seg = n
			sg.units.sort(key=lambda u: (u.name != ".text.start", u.addr, u.index))
		over, plan = layout(segs, multi, units, refs, dat, shadow, ovf, gate_syms)
		if not over:
			break
		for n, size in over.items():
			sg = segs[n]
			if not sg.auto or len(sg.units) <= 1:
				raise PackError(f"code segment {n} is {size} bytes with its veneers, over 64 KB; move code out of it")
			placed = sum(u.size for u in sg.units)
			caps[n] = min(sg.budget, placed) - (size - MAX_SEG) - 256
	else:
		raise PackError("could not fit the code in 64 KB segments")

	if multi:
		for u in units:
			if u.exports and u.seg != 1:
				raise PackError(f"exported procedure {u.label()} is in segment {u.seg}; exported procedures live in segment 1")
	if len(dat) + stack + heap > 0x10000:
		raise PackError(f"data {len(dat)} + stack {stack} + heap {heap} is over one 64 KB segment")

	numbers = sorted(segs)
	code_images = plan["images"]
	seg_relocs = plan["relocs"]

	def place(name):
		u, off = code_loc(name)
		return u.seg, u.base + off

	imports = []  # (module, ordinal, segment, offset)
	exports = []  # (name, offset)
	for s in syms:
		nm = s["name"]
		m = re.fullmatch(r"__w16imp_([A-Z0-9]+)_(\d+)_\w+", nm)
		if m:
			sn, off = place(nm)
			imports.append((m.group(1), int(m.group(2)), sn, off))
			continue
		m = re.fullmatch(r"__w16exp_(\w+)", nm)
		if m:
			sn, off = place(nm)
			if sn != 1:
				raise PackError(f"{nm} is in segment {sn}; exported procedures live in segment 1")
			exports.append((m.group(1), off))
			continue
		m = re.fullmatch(r"__w16sel_(code|data|seg(\d+))_\w+", nm)
		if m:
			sn, off = place(nm)
			target = 1 if m.group(1) == "code" else DATA_SEG if m.group(1) == "data" else int(m.group(2))
			if target != DATA_SEG and target not in segs:
				raise PackError(f"{nm} names segment {target}, which does not exist")
			struct.pack_into("<H", code_images[sn], off, 0xFFFF)
			seg_relocs[sn].append((off, struct.pack("<BBHBBH", RELOC_SELECTOR, 0, off, target, 0, 0)))
	exports.sort()
	imports.sort(key=lambda r: (r[2], r[3]))
	mod_names = []
	for mn, _o, _s, _v in imports:
		if mn not in mod_names:
			mod_names.append(mn)
	core = ("KERNEL", "USER", "GDI")
	mod_names.sort(key=lambda n: core.index(n) if n in core else len(core))  # stable: other modules keep their first-seen order

	# A fixup that is not additive is a link in a chain: the loader reads the offset of the next fixup from the location it is about to
	# patch, and 0xFFFF ends the chain. Each site here is a chain of one.
	for mn, o, sn, off in imports:
		struct.pack_into("<H", code_images[sn], off, 0xFFFF)
		seg_relocs[sn].append((off, struct.pack("<BBHHH", RELOC_FAR, 1, off, mod_names.index(mn) + 1, o)))
	reloc_blobs = {}
	for n in numbers:
		rs = sorted(seg_relocs[n], key=lambda r: r[0])
		if len(rs) > 0xFFFF:
			raise PackError(f"code segment {n} needs {len(rs)} fixups, more than 65535")
		reloc_blobs[n] = struct.pack("<H", len(rs)) + b"".join(r[1] for r in rs)

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
	for i in range(0, len(exports), 255):
		chunk = exports[i : i + 255]
		entry += bytes([len(chunk), 1])  # a bundle of fixed entries in segment 1
		for _n, off in chunk:
			entry += struct.pack("<BH", 0x03, off)
	entry += b"\0\0"

	# Layout. The MZ stub is 0x80 bytes, the NE header 0x40, then the tables, each directly after the last. The segment table lists the
	# code segment 1, the data segment 2 and then code segments 3 and up; their contents follow the tables in that order.
	table_order = [1, DATA_SEG] + [n for n in numbers if n != 1]
	ne_off = 0x80
	seg_tab = ne_off + 0x40
	res_tab = seg_tab + 8 * len(table_order)
	res_blob_dummy, _ = resource_table(resources, 0) if resources else (b"", b"")
	resname_tab = res_tab + len(res_blob_dummy)
	modref_tab = resname_tab + len(resident)
	impname_tab = modref_tab + len(modref)
	entry_tab = impname_tab + len(imp_names)
	nonres_tab = entry_tab + len(entry)
	pos = align(nonres_tab + len(nonres), 1 << FILE_SHIFT)
	file_off = {}
	for n in table_order:
		file_off[n] = pos
		size = len(dat) if n == DATA_SEG else len(code_images[n]) + len(reloc_blobs[n])
		pos = align(pos + size, 1 << FILE_SHIFT)
	if pos >> FILE_SHIFT > 0xFFFF:
		raise PackError("the program is over 1 MB, more than a 16-byte file alignment can address")
	res_data_base = align(pos, 1 << RES_SHIFT)
	res_blob, res_data = resource_table(resources, res_data_base) if resources else (b"", b"")

	seg_table = b""
	for n in table_order:
		if n == DATA_SEG:
			seg_table += struct.pack("<HHHH", file_off[n] >> FILE_SHIFT, len(dat), SEG_DATA | SEG_PRELOAD | SEG_RING3, len(dat))
		else:
			seg_table += struct.pack("<HHHH", file_off[n] >> FILE_SHIFT, len(code_images[n]), segs[n].flags(), len(code_images[n]))

	start_seg, start_ip = place("_start")
	hdr = bytearray(0x40)
	hdr[0:2] = b"NE"
	hdr[2] = 5
	hdr[3] = 1
	struct.pack_into("<HH", hdr, 4, entry_tab - ne_off, len(entry))
	struct.pack_into("<I", hdr, 8, 0)
	struct.pack_into("<HHHH", hdr, 0x0C, NE_WINAPI | NE_MULTIPLEDATA, DATA_SEG, heap, stack)
	struct.pack_into("<HHHH", hdr, 0x14, start_ip, start_seg, 0, DATA_SEG)
	struct.pack_into("<HHHH", hdr, 0x1C, len(table_order), len(mod_names), len(nonres), seg_tab - ne_off)
	struct.pack_into("<HHHH", hdr, 0x24, res_tab - ne_off, resname_tab - ne_off, modref_tab - ne_off, impname_tab - ne_off)
	struct.pack_into("<I", hdr, 0x2C, nonres_tab)
	struct.pack_into("<HH", hdr, 0x30, 0, FILE_SHIFT)
	struct.pack_into("<H", hdr, 0x34, len(resources))
	hdr[0x36] = 2  # Windows
	struct.pack_into("<H", hdr, 0x3E, expected_ver)
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
	out += seg_table
	out += res_blob
	out += resident + modref + imp_names + entry + nonres
	for n in table_order:
		out += bytes(file_off[n] - len(out))
		out += dat if n == DATA_SEG else code_images[n] + reloc_blobs[n]
	if resources:
		out += bytes(res_data_base - len(out))
		out += res_data
	info = {
		"code": sum(len(code_images[n]) for n in numbers),
		"data": len(dat),
		"stack": stack,
		"heap": heap,
		"imports": len(imports),
		"exports": [n for n, _o in exports],
		"modules": mod_names,
		"relocs": sum(len(seg_relocs[n]) for n in numbers),
		"resources": len(resources),
		"file": len(out),
		"segments": [{"number": n, "size": len(code_images[n]), "units": len(segs[n].units), "flags": segs[n].flags(), "name": segs[n].name} for n in numbers],
		"veneers": plan["veneers"],
		"slots": plan["slots"],
		"stubs": plan["stubs"],
		"shadow": plan["shadow"],
		"map": plan["map"],
	}
	return bytes(out), info


def layout(segs, multi, units, refs, dat, shadow, ovf, gate_syms):
	"""Lays out every segment and writes the code. Returns ({segment: size} for segments over 64 KB, None) or ({}, the plan)."""
	numbers = sorted(segs)
	# Classify each reference now that every unit has a segment.
	slots = {}
	calls = []
	jumps = []
	for r in refs:
		r.kind = None
		tu = r.target
		if r.pc:
			if r.where == "data":
				raise PackError(f"a relative reference to code ({r.sym}) at {r.site()}")
			if r.unit.seg == tu.seg:
				r.kind = "rel"
				continue
			kind = branch_kind(r.unit.code, r.off, r.size)
			if kind == "call":
				r.kind = "call"
				calls.append(r)
			elif kind == "jump":
				r.kind = "jump"
				jumps.append(r)
			elif kind == "call16":
				raise PackError(f"a 16-bit near call from {r.site()} to {r.sym} in another segment; only 32-bit calls can go through a veneer")
			else:
				raise PackError(f"{RELOC_NAMES[r.typ]} at {r.site()} reaches {r.sym} in segment {tu.seg} and is not a call or a jump")
			continue
		if multi and r.ref in tu.func_starts and r.ref not in tu.exports:
			r.kind = "slot"
			slots.setdefault((tu.index, r.ref), (tu, r.ref))
			continue
		if multi and r.where == "code" and r.unit.seg != tu.seg and r.ref not in tu.exports:
			raise PackError(f"{r.site()} takes the address {r.sym}+{r.ref:#x}, inside a function in segment {tu.seg}; only a function's start can be used from another segment")
		r.kind = "abs"
	slot_list = sorted(slots.values(), key=lambda t: (t[0].seg, t[0].addr, t[1]))
	slot_index = {(t[0].index, t[1]): k for k, t in enumerate(slot_list)}

	# The shadow return stack lives in the data segment, after the program's own data.
	ssp = limit = 0
	extra = b""
	if multi:
		ssp = align(len(dat), 4)
		base = ssp + 4
		limit = base + 4 * shadow
		extra = bytes(ssp - len(dat)) + p16(base) + bytes(2) + bytes(4 * shadow)
	# A segment that is loaded on call or discardable may be absent when control reaches it, so every far entry into it goes through
	# the gate in segment 1 (see gate_code).
	gated = {n for n in numbers if n != 1 and (not segs[n].preload or segs[n].discardable)}
	if gated and gate_syms is None:
		raise PackError(f"segment {min(gated)} is loaded on call or discardable, which needs the gate's symbols from the w16 runtime (__w16_ss16, __w16_ss32, w16_sync_alias)")
	header, tramp, dyn, header_sites = shared_header(ssp, limit, ovf is not None, bool(gated)) if multi else (b"", 0, 0, [])
	slot_base = len(header)
	units_start = align(slot_base + SLOT_SIZE * len(slot_list), 4) if multi else 0

	# Offsets: units first, then veneers in the caller's segment, return trampolines in the callee's and jump stubs in the jumper's.
	ends = {}
	for n in numbers:
		cur = units_start
		for u in segs[n].units:
			cur = align(cur, u.align)
			u.base = cur
			cur += u.size
		ends[n] = cur
	gate_at = None
	if gated:
		gate_at = ends[1]
		ends[1] += len(gate_code(0, 0, lambda at: 0))
	veneer_at = {}
	tramp_at = {}
	stub_at = {}
	for r in calls:
		veneer_at[id(r)] = ends[r.unit.seg]
		ends[r.unit.seg] += 17 if r.target.seg in gated else 11
		tramp_at[id(r)] = ends[r.target.seg]
		ends[r.target.seg] += 11 if r.unit.seg in gated else 5
	for r in jumps:
		key = (r.unit.seg, r.target.index, r.ref)
		if key not in stub_at:
			stub_at[key] = ends[r.unit.seg]
			ends[r.unit.seg] += 9
	over = {n: ends[n] for n in numbers if ends[n] > MAX_SEG}
	if over:
		return over, None

	images = {n: bytearray(b"\xcc" * ends[n]) for n in numbers}
	relocs = {n: [] for n in numbers}

	def far_fixup(n, site, seg, off):
		struct.pack_into("<HH", images[n], site, 0xFFFF, 0)
		relocs[n].append((site, struct.pack("<BBHBBH", RELOC_FAR, 0, site, seg, 0, off)))

	def sel_fixup(n, site, seg):
		struct.pack_into("<H", images[n], site, 0xFFFF)
		relocs[n].append((site, struct.pack("<BBHBBH", RELOC_SELECTOR, 0, site, seg, 0, 0)))

	def dyn_jump(n, at, seg, off):
		"""A 9-byte stub that pushes a far target and enters the dynamic veneer."""
		images[n][at : at + 9] = bytes([0x68, 0xFF, 0xFF, 0x68]) + p16(off) + b"\xe9" + p16(dyn - (at + 9))
		sel_fixup(n, at + 1, seg)

	def far_jump(n, at, seg, off):
		"""A far jump to seg:off, through the gate when that segment may be absent. Returns its length (5 or 11)."""
		if seg in gated:
			images[n][at : at + 6] = bytes([0x68, 0xFF, 0xFF, 0x68]) + p16(off)
			sel_fixup(n, at + 1, seg)
			at += 6
		images[n][at] = 0xEA
		far_fixup(n, at + 1, *((1, gate_at) if seg in gated else (seg, off)))
		return 11 if seg in gated else 5

	if gated:
		sync_u, sync_off = gate_syms["sync"]
		images[1][gate_at : gate_at + len(gate_code(0, 0, lambda at: 0))] = gate_code(
			gate_syms["ss16"], gate_syms["ss32"], lambda at: sync_u.base + sync_off - (gate_at + at)
		)
	if multi:
		ovf_target = (ovf[0].seg, ovf[0].base + ovf[1]) if ovf else None
		for n in numbers:
			images[n][0 : len(header)] = header
			for site, what in header_sites:
				far_fixup(n, site, *(ovf_target if what == "overflow" else (1, gate_at)))
			for k, (tu, ref) in enumerate(slot_list):
				at = slot_base + SLOT_SIZE * k
				if tu.seg == n:
					images[n][at : at + 3] = b"\xe9" + p16(tu.base + ref - (at + 3))
				else:
					dyn_jump(n, at, tu.seg, tu.base + ref)
	for u in units:
		images[u.seg][u.base : u.base + u.size] = u.code

	for r in refs:
		tu = r.target
		dest = tu.base + r.ref
		if r.where == "code":
			buf, n, p = images[r.unit.seg], r.unit.seg, r.unit.base + r.off
		else:
			buf, n, p = dat, None, r.off
		what = f"{RELOC_NAMES[r.typ]} at {r.site()} to {r.sym}"
		if r.kind == "rel":
			write_field(buf, p, r.size, dest - (p + r.size), True, what)
		elif r.kind == "call":
			v = veneer_at[id(r)]
			t = tramp_at[id(r)]
			buf[p - 1] = 0xE9  # the call becomes a jump to the veneer, which pushes the return trampoline as the return address
			write_field(buf, p, 4, v - (p + 4), True, what)
			images[n][v : v + 6] = bytes([0x66, 0x68]) + struct.pack("<I", t)
			far_jump(n, v + 6, tu.seg, dest)
			far_jump(tu.seg, t, n, p + 4)
		elif r.kind == "jump":
			at = stub_at[(n, tu.index, r.ref)]
			write_field(buf, p, r.size, at - (p + r.size), True, what)
		elif r.kind == "slot":
			write_field(buf, p, r.size, slot_base + SLOT_SIZE * slot_index[(tu.index, r.ref)], False, what)
		else:
			write_field(buf, p, r.size, dest, False, what)
	for (n, ti, ref), at in stub_at.items():
		tu = next(u for u in units if u.index == ti)
		dyn_jump(n, at, tu.seg, tu.base + ref)

	if multi:
		dat += extra
	mp = []
	for n in numbers:
		for u in segs[n].units:
			mp.append((n, u.base, u.size, u.label()))
		if multi:
			mp.append((n, 0, slot_base, "(shared veneer and return trampoline)"))
			if slot_list:
				mp.append((n, slot_base, SLOT_SIZE * len(slot_list), "(function pointer slots)"))
	return {}, {
		"images": images,
		"relocs": relocs,
		"veneers": len(calls),
		"stubs": len(stub_at),
		"slots": [(tu.label() if ref == 0 else f"{tu.label()}+{ref:#x}", slot_base + SLOT_SIZE * k) for k, (tu, ref) in enumerate(slot_list)],
		"shadow": (ssp, shadow) if multi else None,
		"map": sorted(mp),
	}


def write_map(path, info):
	with open(path, "w", encoding="utf-8", newline="\n") as f:
		for n, off, size, name in info["map"]:
			f.write(f"{n:04X}:{off:04X} {size:6d} {name}\n")


def main():
	args = sys.argv[1:]
	opts = {"module": "SETUP", "description": "", "stack": 0x2000, "heap": 0x1000}
	res_path = seg_path = map_path = None
	try:
		while args and args[0].startswith("--"):
			o = args.pop(0)
			if o == "--module":
				opts["module"] = args.pop(0)
			elif o == "--description":
				opts["description"] = args.pop(0)
			elif o in ("--stack", "--heap", "--shadow"):
				opts[o[2:]] = int(args.pop(0), 0)
			elif o == "--resources":
				res_path = args.pop(0)
			elif o == "--segments":
				seg_path = args.pop(0)
			elif o == "--map":
				map_path = args.pop(0)
			else:
				sys.exit("ne_pack: unknown option " + o + "\n" + USAGE)
	except (IndexError, ValueError):
		sys.exit(USAGE)
	if len(args) != 2:
		sys.exit(USAGE)
	try:
		seg_cfg = None
		if seg_path:
			with open(seg_path, encoding="utf-8") as f:
				seg_cfg = json.load(f)
		with open(args[0], "rb") as f:
			out, info = pack(f.read(), resources=load_resources(res_path), segments=seg_cfg, **opts)
	except PackError as e:
		sys.exit("ne_pack: " + str(e))
	with open(args[1], "wb") as f:
		f.write(out)
	if map_path:
		write_map(map_path, info)
	segs = ", ".join(f"{s['number']}: {s['size']}" for s in info["segments"])
	extra = ""
	if len(info["segments"]) > 1:
		extra = f"; {info['veneers']} call veneers, {info['stubs']} jump stubs, {len(info['slots'])} pointer slots"
	print(
		f"ne_pack: {args[1]}: {info['file']} bytes (code {info['code']} in segments {segs}{extra}; data {info['data']} + stack {info['stack']} + heap {info['heap']}, {info['imports']} imports, {info['resources']} resources)"
	)


if __name__ == "__main__":
	main()
