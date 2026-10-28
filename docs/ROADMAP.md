# Roadmap

## M1 — Engine (in progress)

- [x] PE32/PE32+ parsing: sections, build timestamp, VA ↔ file offset, packer hints
- [ ] Search: hex with `??` wildcards (done) / nth / last hit / text (cp949, gbk, utf8) / section scoped
- [ ] Write primitives: `GetU8/16/32`, `SetU8/16/32`, `SetHex`, NOP fill, relative `JMP`, resolve jump target
- [ ] Declarative YAML patch table + `apply` / `verify` + JSON report + idempotency + bounds checks
- [ ] Ship an example Chinese (GBK) string-translation table
- [ ] Unit tests + integration test against a client executable supplied via `WARP_RS_TEST_EXE`

## M2 — Launcher GUI

- [ ] Patch list UI (checkboxes, descriptions, per-patch status)
- [ ] Pipeline: check for updates → apply patches → launch client
- [ ] Failure handling: version mismatch warnings, rollback to the unpatched binary

## Later

- [ ] More patch kinds: skip integrity checks, multiple GRFs, lua override paths, license-screen removal
- [ ] Group patch sets by client build date
- [ ] Optional: embed the engine as a library crate behind a stable API
