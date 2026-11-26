# Contributing to Rune Darkroom

Thank you for your interest in contributing to **Rune Darkroom**! Pull requests, bug reports, design polish, camera RAW compatibility improvements, and cross-platform fixes are all welcome.

Please read [docs/ENGINEERING_GUIDELINES.md](docs/ENGINEERING_GUIDELINES.md) for the full architecture, parity, sidecar integrity, and local MCP security standards.

## Getting Started

1. Fork the repository and clone your fork:
   ```bash
   git clone https://github.com/<your-username>/rune-editor.git
   cd rune-editor
   ```
2. Ensure you have a recent Rust toolchain supporting **Edition 2024** (`rustc --version`). On macOS, make sure Xcode Command Line Tools are installed (`xcode-select --install`).
3. Verify everything builds and passes locally (prefer `--release` to keep a single Cargo build profile on disk):
   ```bash
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --release -- -D warnings
   cargo test --workspace --release
   ```

## Workspace & Design Principles

- **`crates/core` stays UI- and GPU-agnostic:** All recipe data structures, spline math, parameter registry, analysis, and reference pixel evaluation live here with zero `wgpu` or `rune` dependencies.
- **GUI ↔ CLI ↔ MCP 1:1 parity:** Register every new develop parameter, preset, or geometry option in `crates/core/src/registry.rs` (`SCALAR_PARAMS`, enforced by `registry_covers_all_recipe_fields`) and `RecipePatch` so it works identically in the GUI, `rd` CLI, and MCP tools, and update `skills/rune-darkroom/SKILL.md` for new workflow capabilities.
- **GPU ↔ CPU color & spatial parity (`< 2e-3`):** Any adjustment in `shaders/develop.wgsl` (including spatial detail, vignette, grain, and geometry) must match `crates/core` + `crates/decode/src/export.rs`, guarded by `gpu_shader_compiles_and_matches_cpu_color_science` and export parity tests. Route all CPU renders and MCP previews through `crates/decode/src/export.rs`.
- **Non-destructive data & MCP (`2025-11-25`) security:** Never overwrite originals (`guard_not_source` + RAW extension blocklist), use atomic writes with monotonic in-place `PhotoMeta.revision` and preserved `history`, and follow the local MCP loopback, `Host`/`Origin`, framing, session, and `out`/`overwrite` security checklist in [docs/ENGINEERING_GUIDELINES.md](docs/ENGINEERING_GUIDELINES.md).
- **Keep `crates/decode` cross-platform friendly:** Put Apple-only accelerations (`CIRAWFilter`, `ImageIO`) behind `#[cfg(target_os = "macos")]` and keep portable Rust fallbacks (`rawler`, `image`) working on all targets.
- **Respect macOS HIG & OpenCode aesthetics:** Keep the central photo matte strictly color-neutral (`k.c.page` `#121212`), preserve `ChromeInsets` traffic-light clearance in `toolbar.rs`, and use `rune-theme-opencode` tokens rather than hardcoded colors.

## Visual Testing for UI PRs

Rune supports headless GPU rendering without opening a window. For any UI or shader change, verify headless renders across affected scenes:

```bash
for scene in default color tone-curve split filmstrip; do
  cargo run --release -p rune-editor -- --chrome --snap "/tmp/rune-${scene}.png" --scene "$scene"
done
```

Confirm Rune reports `0 overlapping elements` on the macOS traffic lights, attach before/after screenshots to your PR, and delete temporary PNGs before committing.

## Note on Pinned `anomalyco/rune`

Because Rune is currently pre-release and Cargo does not inherit `[patch.crates-io]` from dependencies, the workspace root `Cargo.toml` pins `rune`, `rune-kit`, `rune-theme-opencode`, and the 8 patched crates to the same `anomalyco/rune` git `rev`. If you bump the Rune revision, update all pinned entries together.

## Submitting a Pull Request

1. Create a feature branch from `main` (`git checkout -b feat/my-improvement`).
2. Keep commits focused and clear.
3. Run the verification checklist from [docs/ENGINEERING_GUIDELINES.md](docs/ENGINEERING_GUIDELINES.md):
   - `cargo fmt --all --check`
   - `cargo clippy --workspace --all-targets --release -- -D warnings`
   - `cargo test --workspace --release`
4. Open a Pull Request describing the motivation, behavior changes, and screenshots if UI-facing.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
