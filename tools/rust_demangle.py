#!/usr/bin/env python3
# Turns Rust symbol names (v0 `_R...` and legacy `_ZN...E`) into readable paths, for segment placement patterns and the packer's map.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# usage: tools/rust_demangle.py SYMBOL...
#
# The output follows rustc's own printing closely enough to match by pattern: `w16::Log::dec`, `<bigseg::Square as bigseg::Shape>::area`,
# `core::ptr::drop_in_place::<w16::Log>`. Crate disambiguators and hashes are dropped. A name that does not parse comes back unchanged.
import re
import sys

BASIC = {
	"a": "i8",
	"b": "bool",
	"c": "char",
	"d": "f64",
	"e": "str",
	"f": "f32",
	"h": "u8",
	"i": "isize",
	"j": "usize",
	"l": "i32",
	"m": "u32",
	"n": "i128",
	"o": "u128",
	"p": "_",
	"s": "i16",
	"t": "u16",
	"u": "()",
	"v": "...",
	"x": "i64",
	"y": "u64",
	"z": "!",
}
LEGACY_ESCAPES = {"$SP$": "@", "$BP$": "*", "$RF$": "&", "$LT$": "<", "$GT$": ">", "$LP$": "(", "$RP$": ")", "$C$": ","}


class _Fail(Exception):
	pass


class _V0:
	def __init__(self, s):
		self.s = s
		self.i = 0
		self.depth = 0

	def peek(self):
		return self.s[self.i] if self.i < len(self.s) else ""

	def next(self):
		if self.i >= len(self.s):
			raise _Fail()
		c = self.s[self.i]
		self.i += 1
		return c

	def eat(self, c):
		if self.peek() == c:
			self.i += 1
			return True
		return False

	def base62(self):
		if self.eat("_"):
			return 0
		n = 0
		while True:
			c = self.next()
			if c == "_":
				return n + 1
			if c.isdigit():
				d = ord(c) - 48
			elif "a" <= c <= "z":
				d = ord(c) - 97 + 10
			elif "A" <= c <= "Z":
				d = ord(c) - 65 + 36
			else:
				raise _Fail()
			n = n * 62 + d

	def decimal(self):
		start = self.i
		while self.peek().isdigit():
			self.i += 1
		if start == self.i:
			raise _Fail()
		return int(self.s[start : self.i])

	def disambiguator(self):
		if self.eat("s"):
			self.base62()

	def ident(self):
		self.disambiguator()
		return self.undisambiguated()

	def undisambiguated(self):
		self.eat("u")
		n = self.decimal()
		self.eat("_")
		if self.i + n > len(self.s):
			raise _Fail()
		t = self.s[self.i : self.i + n]
		self.i += n
		return t

	def backref(self, fn):
		target = self.base62()  # positions count from the character after `_R`
		saved = self.i
		self.depth += 1
		if self.depth > 64 or target >= saved:
			raise _Fail()
		self.i = target
		out = fn()
		self.i = saved
		self.depth -= 1
		return out

	def path(self):
		c = self.next()
		if c == "C":
			return self.ident()
		if c == "M":
			self.disambiguator()
			self.path()
			return self.type_()
		if c == "X":
			self.disambiguator()
			self.path()
			t = self.type_()
			tr = self.path()
			return f"<{t} as {tr}>"
		if c == "Y":
			t = self.type_()
			tr = self.path()
			return f"<{t} as {tr}>"
		if c == "N":
			ns = self.next()
			parent = self.path()
			self.disambiguator()
			name = self.undisambiguated()
			if ns.isupper():
				kind = {"C": "closure", "S": "shim"}.get(ns, ns)
				return f"{parent}::{{{kind}}}" if not name else f"{parent}::{{{kind}:{name}}}"
			return f"{parent}::{name}" if name else parent
		if c == "I":
			p = self.path()
			args = []
			while not self.eat("E"):
				args.append(self.generic_arg())
			args = [a for a in args if a]
			return f"{p}::<{', '.join(args)}>" if args else p
		if c == "B":
			return self.backref(self.path)
		raise _Fail()

	def generic_arg(self):
		if self.eat("L"):
			self.base62()
			return ""
		if self.eat("K"):
			return self.const()
		return self.type_()

	def const(self):
		if self.eat("B"):
			return self.backref(self.const)
		if self.eat("p"):
			return "_"
		t = self.next()
		if t not in BASIC:
			raise _Fail()
		neg = self.eat("n")
		start = self.i
		while self.peek() != "_":
			self.next()
		digits = self.s[start : self.i]
		self.next()
		v = int(digits, 16) if digits else 0
		if t == "b":
			return "true" if v else "false"
		return ("-" if neg else "") + str(v)

	def type_(self):
		c = self.peek()
		if c in BASIC:
			self.i += 1
			return BASIC[c]
		if c in "CMXYNI":
			return self.path()
		self.i += 1
		if c == "B":
			return self.backref(self.type_)
		if c in "RQ":
			if self.eat("L"):
				self.base62()
			return ("&" if c == "R" else "&mut ") + self.type_()
		if c in "PO":
			return ("*const " if c == "P" else "*mut ") + self.type_()
		if c == "A":
			t = self.type_()
			return f"[{t}; {self.const()}]"
		if c == "S":
			return f"[{self.type_()}]"
		if c == "T":
			ts = []
			while not self.eat("E"):
				ts.append(self.type_())
			return "(" + ", ".join(ts) + ("," if len(ts) == 1 else "") + ")"
		if c == "F":
			if self.eat("G"):
				self.base62()
			self.eat("U")
			if self.eat("K"):
				if not self.eat("C"):
					self.undisambiguated()
			ts = []
			while not self.eat("E"):
				ts.append(self.type_())
			ret = self.type_()
			return f"fn({', '.join(ts)}) -> {ret}"
		if c == "D":
			if self.eat("G"):
				self.base62()
			bounds = []
			while not self.eat("E"):
				b = self.path()
				while self.eat("p"):
					self.undisambiguated()
					self.type_()
				bounds.append(b)
			if self.eat("L"):
				self.base62()
			return "dyn " + " + ".join(bounds)
		raise _Fail()


def _strip_suffix(name):
	# LLVM appends `.26`, `.llvm.1234` or `.cold` to names it clones or renames.
	return re.sub(r"(\.(llvm\.)?\d+|\.cold)+$", "", name)


def demangle(name):
	"""Returns the readable path of a Rust symbol, or `name` unchanged when it is not one."""
	base = _strip_suffix(name)
	if base.startswith("_R"):
		p = _V0(base[2:])
		try:
			if p.peek().isdigit():
				p.decimal()
			return p.path()
		except (_Fail, IndexError, ValueError, RecursionError):
			return name
	if base.startswith("_ZN") and base.endswith("E"):
		parts = []
		i = 3
		body = base[:-1]
		try:
			while i < len(body):
				m = re.match(r"\d+", body[i:])
				if not m:
					return name
				n = int(m.group(0))
				i += len(m.group(0))
				parts.append(body[i : i + n])
				i += n
		except ValueError:
			return name
		if parts and re.fullmatch(r"h[0-9a-f]{16}", parts[-1]):
			parts.pop()
		out = []
		for part in parts:
			if part.startswith("_$"):
				part = part[1:]
			for k, v in LEGACY_ESCAPES.items():
				part = part.replace(k, v)
			part = re.sub(r"\$u([0-9a-f]+)\$", lambda m: chr(int(m.group(1), 16)), part)
			out.append(part.replace("..", "::"))
		return "::".join(out)
	return name


if __name__ == "__main__":
	for a in sys.argv[1:]:
		print(demangle(a))
