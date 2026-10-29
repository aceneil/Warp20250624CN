# Warp20250624CN

**English** | [中文](README.zh-CN.md)

A **Rust reimplementation of WARP's patch engine** for 32-bit Ragnarok Online clients:
locate byte signatures inside a client executable, overwrite them in place, driven by a
declarative patch table.

WARP (Neo-Mind/WARP, GPL-3.0) is a Qt GUI plus a set of `.qjs` scripts that do
"find signature → overwrite bytes". This project reimplements the engine in Rust so it can
be embedded in a launcher, driven from a CLI, and cross-compiled without Qt.

> Independent implementation. Only WARP's documented patch behaviour was used as a
> reference; no C++ or `.qjs` source was copied.

## Features

| | |
|---|---|
| PE32 / PE32+ parsing | sections, build timestamp, VA ↔ file offset, packer hints |
| Signature search | hex with `??` wildcards, text in cp949 / gbk / utf8 / sjis / cp874, `nth` / `last` / all hits, section scoped |
| Write primitives | `SetHex`, `SetU8/16/32`, NOP fill, relative JMP, offset-addressed writes |
| Declarative tables | YAML patch table → `apply` / `verify` with a JSON report |
| Safety | strict bounds checks, idempotency (already-patched detection), all-or-nothing write |
| Offset mode | fixed-offset writes with original-byte verification (for code caves and PE header fields that cannot be found by signature) |
| Library use | `warp_rs` is a `lib` + `bin`; a launcher can depend on it directly |

## Install

### Desktop app (recommended)

Every tagged release ships the **GUI** only — a Tauri v2 front-end, one codebase for
both platforms:

| platform | asset | runtime requirement |
|---|---|---|
| Linux x86-64 | `warp-gui-linux-x86_64.tar.gz` | `webkit2gtk-4.1` |
| Windows x86-64 | `warp-gui-windows-x86_64.zip` | WebView2 (built into Win10/11) |

Pick a client executable, tick the patches you want, preview, then apply. The app backs up
the original as `<name>.bak` and verifies the result. Patch tables come from `patches/*.yml`
next to the app — no remote profile is needed.

```bash
# build the GUI yourself
cargo build --release --features gui --manifest-path tools/warp_gui/Cargo.toml
```

### Command line

```bash
cargo build --release            # → target/release/warp_rs
```

Windows cross-compilation from Linux uses [`cargo-xwin`](https://github.com/rust-cross/cargo-xwin):

```bash
cargo xwin build --release --target x86_64-pc-windows-msvc
```

## Usage

```bash
# inspect the target executable
warp_rs info --exe Ragexe.exe

# search a signature (hex with wildcards, or text)
warp_rs find --exe Ragexe.exe --hex "B8 DE BD C3 C1 F6"

# apply a patch table, write a new file, emit a JSON report
warp_rs apply --exe Ragexe.exe --patches patches/zh-cn.yml \
        --out Ragexe.patched.exe --report report.json

# re-check the result against the same table
warp_rs verify --exe Ragexe.patched.exe --patches patches/zh-cn.yml

# list language packs (for a launcher's dropdown)
warp_rs langs --dir patches/lang
```

## Patch table

```yaml
version: 1
patches:
  - id: example_string
    desc: Example string replacement
    find: "B8 DE BD C3 C1 F6 00"
    replace: "CF FB CF A2 00"   # raw bytes in the client's codepage
    section: .rdata             # optional: restrict the search range
    expect: 1                   # expected number of hits
    mode: replace               # replace | nop | jmp | offset
```

Notes:

- A replacement may not be longer than the signature, except that trailing `0x00` padding
  directly after the match may be reused (the engine never overwrites non-zero bytes).
- For non-ASCII text, supply bytes in the client's codepage as hex — that sidesteps every
  encoding problem in the tool chain.

### Offset mode

Some reference implementations write where a signature cannot lead you: code caves, global
pointers, PE header fields. For those, address the location directly and verify the
original bytes:

```yaml
  - id: parity-0042
    mode: offset
    offset:  "0x0095B1DC"
    verify:  "A1 00 46 57 01"     # refuse to write if the client build differs
    replace: "E9 10 00 00 00"
```

### Reproducing a known-good output

If a reference build is known to work and a semantic table does not reproduce it, generate
an offset table by diffing the two executables. Applying it produces a **byte-identical**
file:

```bash
scripts/gen-parity-table.sh pristine.exe reference.exe patches/parity.yml
warp_rs apply --exe pristine.exe --patches patches/parity.yml --out out.exe
md5sum out.exe reference.exe      # equal
```

## Scope

- Works on executables the operator is licensed to modify. No game client is bundled or
  redistributed here.
- Test fixtures live outside the repository and are referenced through the
  `WARP_RS_TEST_EXE` environment variable.
- CLI messages are currently in Chinese; English output is on the roadmap.

## License

GPL-3.0 — see [LICENSE](LICENSE). The patch data under `patches/` derives from GPL-3.0
licensed WARP translation tables, so the project as a whole is published under the same
terms.

## Roadmap

See [docs/ROADMAP.md](docs/ROADMAP.md).
