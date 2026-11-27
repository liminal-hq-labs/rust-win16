#!/usr/bin/env bash
# Runs a Win16 program on Windows for Workgroups 3.11 under DOSBox-X, as many times as asked, and counts the runs whose log says PASS.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# Usage: W31_TREE=DIR scripts/run-w31.sh [--runs N] [--log NAME] [--with FILE]... [--system FILE]... PROGRAM.EXE [ARGUMENTS...]
#
# W31_TREE is an installed Windows 3.11 tree (a folder holding WINDOWS/), which you supply: the repository ships no Microsoft files.
# Each run copies the tree to a scratch directory, mounts the copy as C: and a folder holding the program as D:, and starts
# `win /3 D:\PROGRAM.EXE ARGUMENTS`. The program should end Windows when it is done (BIGSEG.EXE does with `/exit`); a run that has not
# ended after WAIT seconds (default 150) is stopped. The run passes when D:\<NAME> (default BIGSEG.LOG) has a `RESULT: PASS` line.
# `--with FILE` puts a file beside the program on D: (a library the program loads, say) and `--system FILE` puts one in
# C:\WINDOWS\SYSTEM of the run's copy of the tree; either may be given more than once. W31_NAME (default w31run) prefixes the DOSBox-X
# container's name, so that runs started by different people do not clash.
# Each run's log is kept as run-N.log in OUT (default a new temporary directory), and a run that had to be stopped leaves a screenshot,
# run-N.png, which shows the fault box if there was one.
#
# DOSBox-X runs from the `dosbox-x` on the path or, failing that, the Docker image DOSBOX_IMAGE (an image with `dosbox-x` on its path).
# It needs an X display: DISPLAY, or Xvfb on W31_DISPLAY (default :58), which the script starts if nothing listens there. Screenshots
# need ImageMagick's `import`.

runs=1
logname=BIGSEG.LOG
with_files=()
system_files=()
while [ $# -gt 0 ]; do
	case "$1" in
	--runs) runs="$2"; shift 2 ;;
	--log) logname="$2"; shift 2 ;;
	--with) with_files+=("$2"); shift 2 ;;
	--system) system_files+=("$2"); shift 2 ;;
	*) break ;;
	esac
done
[ $# -ge 1 ] || { sed -n '9,20p' "$0" >&2; exit 2; }
prog="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
shift
: "${W31_TREE:?set W31_TREE to an installed Windows 3.11 tree}"
wait_s="${WAIT:-150}"
out="${OUT:-$(mktemp -d)}"
mkdir -p "$out"
work="$(mktemp -d)"
name="${W31_NAME:-w31run}-$$"
cleanup() {
	docker rm -f "$name" > /dev/null 2>&1 || true
	rm -rf "$work"
}
trap cleanup EXIT

if [ -z "${DISPLAY:-}" ]; then
	export DISPLAY="${W31_DISPLAY:-:58}"
	if [ ! -S "/tmp/.X11-unix/X${DISPLAY#:}" ]; then
		Xvfb "$DISPLAY" -screen 0 1024x768x24 > "$out/xvfb.log" 2>&1 &
		sleep 1
	fi
fi

exe="$(basename "$prog" | tr '[:lower:]' '[:upper:]')"
[[ "$exe" =~ ^[A-Z0-9_]{1,8}\.EXE$ ]] || { echo "run-w31: $exe is not an 8.3 name, which Windows 3.11 needs" >&2; exit 2; }
pass=0
for i in $(seq 1 "$runs"); do
	rm -rf "$work/c" "$work/d" "$work/conf"
	mkdir -p "$work/d" "$work/conf"
	cp -a --reflink=auto "$W31_TREE" "$work/c"
	cp "$prog" "$work/d/$exe"
	for extra in ${with_files[@]+"${with_files[@]}"}; do
		cp "$extra" "$work/d/$(basename "$extra" | tr '[:lower:]' '[:upper:]')"
	done
	for extra in ${system_files[@]+"${system_files[@]}"}; do
		cp "$extra" "$work/c/WINDOWS/SYSTEM/$(basename "$extra" | tr '[:lower:]' '[:upper:]')"
	done
	{
		printf '%s\n' '[sdl]' 'output=surface' 'fullscreen=false' '[dosbox]' 'machine=svga_s3' 'memsize=32' '[cpu]' 'core=normal' 'cputype=pentium'
		printf '%s\n' 'cycles=max' '[dos]' 'ems=false' 'xms=true' 'umb=true' '[autoexec]' '@echo off' 'mount c /c' 'mount d /d' 'c:'
		printf '%s\n' 'set PATH=c:\windows' 'cd \windows' "win /3 D:\\$exe $*" 'exit'
	} > "$work/conf/run.conf"
	if command -v dosbox-x > /dev/null; then
		(cd "$work" && SDL_AUDIODRIVER=dummy dosbox-x -conf conf/run.conf -nomenu -fastlaunch > "$out/dosbox-$i.log" 2>&1) &
		pid=$!
		running() { kill -0 "$pid" 2> /dev/null; }
		stop() { kill "$pid" 2> /dev/null || true; }
	else
		docker rm -f "$name" > /dev/null 2>&1 || true
		docker run -d --name "$name" --user "$(id -u):$(id -g)" --network host -v /tmp/.X11-unix:/tmp/.X11-unix -e DISPLAY="$DISPLAY" \
			-e SDL_AUDIODRIVER=dummy -e HOME=/tmp -v "$work/c:/c" -v "$work/d:/d" -v "$work/conf:/conf" --entrypoint dosbox-x \
			"${DOSBOX_IMAGE:?set DOSBOX_IMAGE, or put dosbox-x on the path}" -conf /conf/run.conf -nomenu -fastlaunch > /dev/null
		running() { [ "$(docker inspect -f '{{.State.Running}}' "$name" 2> /dev/null)" = true ]; }
		stop() { docker rm -f "$name" > /dev/null 2>&1 || true; }
	fi
	t=0
	while running && [ "$t" -lt "$wait_s" ]; do
		sleep 1
		t=$((t + 1))
	done
	ended=yes
	if running; then
		ended=no
		import -window root "$out/run-$i.png" 2> /dev/null || true
	fi
	stop
	log="$work/d/$logname"
	if [ -f "$log" ]; then
		tr -d '\r' < "$log" > "$out/run-$i.log"
	else
		: > "$out/run-$i.log"
	fi
	if grep -q '^RESULT: PASS' "$out/run-$i.log"; then
		pass=$((pass + 1))
		verdict=PASS
	else
		verdict=FAIL
	fi
	echo "run $i: $verdict (Windows ended: $ended, ${t}s)"
done
echo "run-w31: $pass of $runs passed; logs in $out"
[ "$pass" -eq "$runs" ]
