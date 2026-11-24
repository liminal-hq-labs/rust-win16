#!/usr/bin/env bash
# Runs SEGMOVE.EXE on Windows for Workgroups 3.11 under DOSBox-X in each variant, many times, and reports passes and data segment moves.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# Usage: W31_TREE=DIR [DOSBOX_IMAGE=IMAGE] scripts/run-segmove.sh [--runs N] [--variants "LIST"] out/SEGMOVE.EXE
#
# Each variant is a command line for SEGMOVE.EXE (see examples/segmove/main.rs; `/exit` is added so that Windows ends), and each run is a fresh DOSBox-X session through
# scripts/run-w31.sh. The default variants are `/mb`, `/cd`, `/mb /dsrealloc` and `/cd /dsrealloc`, separated by `;` in --variants.
# W31_TREE is an installed Windows 3.11 tree you supply. A copy of it is made without the Startup group (otherwise whatever that group
# starts runs beside the test), so the tree itself is never changed. OUT (default a new temporary directory) keeps every run's log.
#
# The result per variant is the runs that passed (every byte and the alias intact), the runs whose data segment moved during the
# blocking call (the `MOVES:` line, which is the runtime's own count) and the runs where the control alias, an independent witness,
# agreed with that count. The script fails when a run fails or when no run of a variant saw a move, because a pass without a move proves
# nothing.

runs=20
variants="/mb;/cd;/mb /dsrealloc;/cd /dsrealloc"
while [ $# -gt 0 ]; do
	case "$1" in
	--runs) runs="$2"; shift 2 ;;
	--variants) variants="$2"; shift 2 ;;
	*) break ;;
	esac
done
[ $# -eq 1 ] || { sed -n '9,11p' "$0" >&2; exit 2; }
exe="$1"
: "${W31_TREE:?set W31_TREE to an installed Windows 3.11 tree}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="${OUT:-$(mktemp -d)}"
mkdir -p "$out"
tree="$(mktemp -d)"
trap 'rm -rf "$tree"' EXIT
cp -a --reflink=auto "$W31_TREE/." "$tree"
if [ -f "$tree/WINDOWS/PROGMAN.INI" ]; then
	sed -i '/STARTUP\.GRP/Id' "$tree/WINDOWS/PROGMAN.INI"
fi

status=0
IFS=';' read -r -a list <<< "$variants"
for v in "${list[@]}"; do
	dir="$out/$(echo "$v" | tr -c 'A-Za-z0-9\n' '_' | sed 's/^_*//; s/_*$//')"
	mkdir -p "$dir"
	# shellcheck disable=SC2086
	W31_TREE="$tree" OUT="$dir" bash "$here/run-w31.sh" --runs "$runs" --log SEGMOVE.LOG "$exe" $v /exit > "$dir/runs.txt" || true
	pass=0
	moved=0
	agree=0
	dist=""
	for i in $(seq 1 "$runs"); do
		log="$dir/run-$i.log"
		grep -q '^RESULT: PASS' "$log" && pass=$((pass + 1))
		n="$(sed -n 's/^MOVES: \([0-9]*\).*/\1/p' "$log" | head -1)"
		dist="$dist ${n:--}"
		[ "${n:-0}" -gt 0 ] && moved=$((moved + 1))
		grep -q '(agree)' "$log" && agree=$((agree + 1))
	done
	echo "variant '$v': $pass of $runs passed; data segment moved in the blocking call in $moved; control alias agreed in $agree"
	echo "  moves per run:$dist"
	if [ "$pass" -ne "$runs" ]; then
		status=1
	fi
	if [ "$moved" -eq 0 ]; then
		echo "  no move was forced: the pass proves nothing" >&2
		status=1
	fi
done
echo "logs in $out"
exit "$status"
