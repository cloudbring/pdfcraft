# Ticket #70: original PdfCraft outputs

Captured on 2026-11-03 **before deleting the original implementations**, from pristine
PdfCraft HEAD `3936f93a1479366a951206fc6d3094680fe2d3dc` in the supplied worktree.
The complete fixture directory is below 200 KB. No Adobe software, files or implementation
sources were used.

- `layers.stdout`, `parity.stdout`, `parity-json.stdout`, `help.stderr`: the original
  xtask binary on the current repository, built with `cargo build -p xtask` only.
- `metadata.json`: the same tree\x27s `cargo metadata --no-deps`, reduced to package IDs,
  names, membership and dependencies, with checkout paths replaced by `/workspace`.
- `version-manifest-before.toml` and `version-manifest-after.toml`: the real workspace
  `Cargo.toml` before and after the original `version::replace` to `1.2.3-rc.1`, showing
  all 33 internal `pdfcraft-*` path dependency version pins updated alongside
  `[workspace.package]`. Verified via dry-run `version::compute` (`Plan`).
- `version-before.toml`, `version-after.toml`, `version-after.lock`, `version.stdout` and
  `version-set.stdout`: a dependency-free temporary workspace with path-pin entries,
  captured by running the original `version.rs` to perform a real offline lockfile update.
- `sha256.stdout`: the original `assets::sha256_hex` digests of empty bytes, `abc`,
  `font bytes`, `png` and 1000 ASCII `a` bytes, compared against both `craft-hash` and
  streaming `craft_xtask::sha256_hex`.
