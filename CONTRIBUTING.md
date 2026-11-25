# Contributing to Rune Darkroom

Thank you for your interest in contributing to **Rune Darkroom**! Pull requests, bug reports, design polish, camera RAW compatibility improvements, and cross-platform fixes are all welcome.

## Getting Started

1. Fork the repository and clone your fork:
   ```bash
   git clone https://github.com/<your-username>/rune-editor.git
   cd rune-editor
   ```
2. Ensure you have a recent Rust toolchain supporting **Edition 2024** (`rustc --version`). On macOS, make sure Xcode Command Line Tools are installed (`xcode-select --install`).
3. Verify everything builds and passes locally:
   ```bash
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   ```

## Workspace & Design Principles

- **`crates/core` stays UI- and GPU-agnostic:** All recipe data structures, spline math, session state, and reference pixel evaluation live here with zero `wgpu` or `rune` dependencies.
- **Keep GPU & CPU color science in parity:** Any adjustment added to `shaders/develop.wgsl` must also be reflected in `crates/core` (`CompiledRecipe::evaluate_pixel_scene_linear` + `filmic_tonemap`) so the live histogram and full-resolution exports match the viewport. The engine crate includes a headless GPU-vs-CPU parity test (`gpu_shader_compiles_and_matches_cpu_color_science`) to guard this automatically.
- **Keep `crates/decode` cross-platform friendly:** Put Apple-only accelerations (`CIRAWFilter`, `ImageIO`) behind `#[cfg(target_os = "macos")]` and keep portable Rust fallbacks (`rawler`, `image`) working on all targets.
- **Respect macOS HIG & OpenCode aesthetics:** Keep the central photo matte strictly color-neutral (`k.c.page` `#121212`), preserve `ChromeInsets` traffic-light clearance in `toolbar.rs`, and use `rune-theme-opencode` tokens rather than hardcoded colors.

## Visual Testing for UI PRs

Rune supports headless GPU rendering without opening a window. For any UI or shader change, please verify headless renders across affected scenes:

```bash
cargo run -p rune-editor -- --chrome --snap /tmp/default.png --scene default
cargo run -p rune-editor -- --chrome --snap /tmp/color.png --scene color
cargo run -p rune-editor -- --chrome --snap /tmp/tone-curve.png --scene tone-curve
cargo run -p rune-editor -- --chrome --snap /tmp/split.png --scene split
cargo run -p rune-editor -- --chrome --snap /tmp/filmstrip.png --scene filmstrip
```

Confirm Rune reports `0 overlapping elements` on the macOS traffic lights and attach before/after screenshots to your PR!

## Note on Pinned `anomalyco/rune`

Because Rune is currently pre-release and Cargo does not inherit `[patch.crates-io]` from dependencies, the workspace root `Cargo.toml` pins `rune`, `rune-kit`, `rune-theme-opencode`, and the 8 patched crates to the same `anomalyco/rune` git `rev`. If you bump the Rune revision, update all pinned entries together.

## Submitting a Pull Request

1. Create a feature branch from `main` (`git checkout -b feat/my-improvement`).
2. Keep commits focused and clear.
3. Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.
4. Open a Pull Request describing the motivation, behavior changes, and screenshots if UI-facing.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
