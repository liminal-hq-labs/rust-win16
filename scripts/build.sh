#!/usr/bin/env bash
# Builds the example programs: Rust to an ELF with rust-lld, then tools/ne_pack.py to an NE executable.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# Usage: scripts/build.sh [--regen]
#
# Writes out/MSGBOX.EXE, out/HELLO.EXE, out/SETUP.EXE, out/BIGSEG.EXE and out/DLLHOST.EXE, and the library out/HELLODLL.DLL. `--regen` rebuilds src/imports.rs from tools/imports.tbl first.
# Set CARGO_TARGET_DIR to keep the build output outside the tree and OUT_DIR to move the executables.

cd "$(dirname "${BASH_SOURCE[0]}")/.."
export RUSTC_BOOTSTRAP=1
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
mkdir -p "$CARGO_TARGET_DIR"

if [ "${1:-}" = "--regen" ]; then
	python3 tools/gen_imports.py tools/imports.tbl src/imports.rs
fi

if ! cargo build --release --workspace 2>"$CARGO_TARGET_DIR/build.log"; then
	cat "$CARGO_TARGET_DIR/build.log" >&2
	exit 1
fi

out="${OUT_DIR:-out}"
mkdir -p "$out"
elf_dir="$CARGO_TARGET_DIR/win16/release"
for prog in msgbox hello setup bigseg dllhost; do
	RES_ARGS=()
	[ -f "examples/$prog/resources.json" ] && RES_ARGS=(--resources "examples/$prog/resources.json")
	[ -f "examples/$prog/segments.json" ] && RES_ARGS+=(--segments "examples/$prog/segments.json")
	upper="$(echo "$prog" | tr '[:lower:]' '[:upper:]')"
	python3 tools/ne_pack.py --module "$upper" --description "Rust Win16 $prog" \
		--map "$out/$upper.MAP" ${RES_ARGS[@]+"${RES_ARGS[@]}"} "$elf_dir/$prog" "$out/$upper.EXE"
	echo "built: $out/$upper.EXE ($(wc -c <"$out/$upper.EXE") bytes)"
done

# A library is packed with its manifest of fixed ordinals; its Rust is built by the `w16dll` package.
python3 tools/ne_pack.py --library examples/hellodll/exports.json --map "$out/HELLODLL.MAP" "$elf_dir/hellodll" "$out/HELLODLL.DLL"
echo "built: $out/HELLODLL.DLL ($(wc -c <"$out/HELLODLL.DLL") bytes)"
# Each export thunk is segment 1 code that has to stay small: under 64 bytes (the map's first column is the size).
awk '$3 ~ /^exp_/ && $2 > 64 { print "export thunk over 64 bytes: " $0; bad = 1 } END { exit bad }' "$out/HELLODLL.MAP"

# The same library with another ABI major version, for the host's version check: HELLODLL2.DLL (rename it to HELLODLL.DLL to use it).
HELLODLL_ABI_MAJOR=2 cargo build --release -p w16dll --bin hellodll 2>"$CARGO_TARGET_DIR/build2.log" || { cat "$CARGO_TARGET_DIR/build2.log" >&2; exit 1; }
python3 tools/ne_pack.py --library examples/hellodll/exports.json "$elf_dir/hellodll" "$out/HELLODLL2.DLL"
echo "built: $out/HELLODLL2.DLL ($(wc -c <"$out/HELLODLL2.DLL") bytes)"
