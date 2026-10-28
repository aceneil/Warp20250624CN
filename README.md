# Warp20250624CN

A **Rust reimplementation of WARP's patch engine** for 32-bit RO clients — locate byte
signatures inside a client executable and overwrite them in place, driven by a declarative
patch table.

WARP (Neo-Mind/WARP, GPL-3.0) ships a Qt GUI plus a set of `.qjs` scripts that perform
"find signature → overwrite bytes". This project reimplements the underlying engine in Rust so
it can be embedded in any launcher, driven from the command line, and cross-compiled without
Qt.

> Independent implementation. Only WARP's public patch semantics and API behaviour were used
> as reference; no C++ or `.qjs` source was copied. Patch data (signatures) is produced by
> scanning a client executable directly.

## Capabilities

| Capability | Status |
|---|---|
| PE32 / PE32+ parsing: sections, build timestamp, VA ↔ file offset, packer hints | ✅ done |
| Signature search: hex with `??` wildcards, text (cp949 / gbk / utf8), nth / last hit, section scoped | 🚧 M1 |
| Write primitives: `SetHex`, `SetU8/16/32`, NOP fill, relative JMP, resolve jump target | 🚧 M1 |
| Declarative YAML patch table + `apply` / `verify` + JSON report | 🚧 M1 |
| Idempotency (already-patched detection) and strict bounds checking | 🚧 M1 |
| Launcher GUI (update → patch → launch) | ⏳ M2 |

## Usage (after M1)

```bash
cargo build --release

# inspect the target executable (PE info, sections, packer hints)
./target/release/warp_rs info --exe Ragexe.exe

# search a signature
./target/release/warp_rs find --exe Ragexe.exe --hex "B8 DE BD C3 C1 F6"

# apply a patch table, write to a new file, emit a JSON report
./target/release/warp_rs apply --exe Ragexe.exe --patches patches/zh-cn.yml \
        --out Ragexe.patched.exe --report report.json

# re-check the patched file against the same table
./target/release/warp_rs verify --exe Ragexe.patched.exe --patches patches/zh-cn.yml
```

Patch table format:

```yaml
version: 1
patches:
  - id: example_string
    desc: Example string replacement
    find: "B8 DE BD C3 C1 F6 00"
    replace: "CF FB CF A2"     # raw bytes, hex (use the client codepage for CJK text)
    section: .data             # optional: restrict the search range
    expect: 1                  # expected number of hits
    mode: replace              # replace | nop | jmp
```

A replacement must not be longer than the signature it overwrites. For non-ASCII text always
supply the bytes as hex in the client's codepage — that sidesteps encoding problems in the
tool chain.

## Scope

- Operates on executable files that the operator is licensed to modify. No game client is
  bundled or redistributed here.
- Test fixtures are expected to live outside the repository and be referenced through the
  `WARP_RS_TEST_EXE` environment variable.

## License

GPL-3.0 — see [LICENSE](LICENSE). The patch data under `patches/` is derived from GPL-3.0
licensed WARP translation tables, so the project as a whole is published under the same terms.

## Roadmap

See [docs/ROADMAP.md](docs/ROADMAP.md).
