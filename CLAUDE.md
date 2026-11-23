# CLAUDE.md

A summary of [AGENTS.md](AGENTS.md), which is canonical. Keep the two in sync.

## What this is

Rust Win16 is a Liminal HQ Labs experiment: Build Windows 3.x programs in Rust: a code16 target, an NE packer and import stubs. Experimental, so anything may change. Behaviour lives in `SPEC.md`, decisions in `docs/decisions.md`.

## Rules to remember

- **Canadian English.** Real em dashes. Sentence case. No hard-wrapped markdown prose. Ship the result, not the journey: no "this PR" or reviewer names in code or docs.
- **Conventional Commits.** Body in markdown with bold labels and no headings, hard-wrapped, written to a file and committed with `git commit -F`. End with a `Co-Authored-By` trailer.
- **Git.** Topic branches, merge commits only, branches deleted on merge. Never push, tag, publish or dispatch workflows unless asked. Never `git add -A` or `git commit -a`. `PLAN.md` stays out of git.
- **Formatting.** Tabs everywhere except YAML. Rust uses rustfmt with hard tabs (`rustfmt.toml`).
- **Headers.** SPDX header on every source, script, YAML, TOML and SVG file: summary line, blank, "(c) Copyright 2026 Liminal HQ, Scott Morris", `SPDX-License-Identifier: Apache-2.0 OR MIT`.
- **Architecture.** Registers first: stubs and `export_proc!` save EBX, ESI and EDI, clear the high half of ESP and reload ES. INITTASK is the first instruction. One 64 KB code segment and one 64 KB data segment. Test packer changes against `tools/ne_dump.py`. No Microsoft files. `src/imports.rs` is generated from `tools/imports.tbl`; never hand-edit it.
- **Toolchain.** Rust 1.98.0 with `RUSTC_BOOTSTRAP=1` and `-Zbuild-std`; Python 3 with no packages, tab-indented. Build with `scripts/build.sh`, outputs outside the tree.
- **CI.** One job per check. Shared images come from `liminal-hq/.github`. Dependabot is off unless the project opts in.
- **Docs.** Update `SPEC.md` and `docs/decisions.md` in the same change as the behaviour.

## Commands

```sh
bash scripts/check-headers.sh
cargo fmt --all -- --check
python3 -m unittest discover -s tests -v
CARGO_TARGET_DIR=/tmp/w16-target OUT_DIR=/tmp/w16-out bash scripts/build.sh
bash scripts/check-banner.sh     # add --shots /tmp/shots for review frames
docker run --rm -v "$PWD:/workdir" davidanson/markdownlint-cli2:v0.17.2 "**/*.md"
```
