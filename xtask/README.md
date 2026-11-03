# PdfCraft xtask

Run `cargo xtask help` for the workspace command table.

This crate is a development tool only. It depends on `craft-xtask` and `craft-hash` from
the `algo/integration` branch of `cloudbring/craft-libs` (`4acbbe2`); repoint both git
dependencies in `xtask/Cargo.toml` to `storytold/craft-libs` after the upstream merge.
Application and library crates must not depend on `craft-xtask`.

## Shared vs. local tasks

- **Shared via `craft-xtask` / `craft-hash`:**
  - `version`: configured in `xtask/src/config.rs` with `update_path_pins: true` to
    preserve PdfCraft\x27s variant behavior of updating matching `version = "..."` pins on
    `path = "..."` entries in `[workspace.dependencies]` alongside `[workspace.package]`.
  - `layers`: L0–L7 table, standalone L0 (`geom`, `filters`, `crypt`, `arlington`) and
    L1 (`cos`) crates, `cos -> filters, crypt, geom` foundation allowlist, sideways edges,
    test support (`testkit`, `oracle`), L7+ UI toolkit rules (`egui_kittest` allowed as a
    dev-dependency below L7), and wasm crate selection are configured in `config.rs` over
    `craft_xtask::layers::compute` / `check_graph` (`Report`), formatted with PdfCraft\x27s
    table layout.
  - `SHA-256`: `assets.rs` uses `craft_xtask::sha256_hex` for streaming file checks and
    re-exports `craft_hash::sha256_hex` for in-memory digests (`corpus`, `license_check`,
    `screenshots`), removing the direct `sha2` dependency.
- **Kept local:**
  - `parity`: validates `parity/acrobat-features.toml` against `crates/engine/src/commands.rs`,
    `crates/automation/src/tools.rs` and test evidence functions, and reports P0–P3 tier and
    A–N area progress (`--json`, `--partial`). Per `SURVEY.md`, this is app-domain TOML and
    source-evidence validation rather than the shared Markdown weighted checklist.
  - `assets`, `corpus`, `demo_pdf`, `fuzz`, `gates`, `license_check`, `macos_integration`,
    `screenshots`: PdfCraft-specific build, verification, fuzzing and asset gates.

## Verification

Only the `xtask` crate needs to be built or tested for tooling changes:

```sh
cargo test -p xtask
cargo clippy -p xtask --all-targets -- -D warnings
cargo fmt --check -p xtask
cargo xtask layers
cargo xtask parity
```
