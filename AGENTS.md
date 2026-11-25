# AGENTS.md — Rune Darkroom (`rune-editor`)

## Project Overview
Mac-first, cross-platform-ready Capture One / Lightroom-style GPU-accelerated photo editor built in Rust on [Anomaly's Rune](https://github.com/anomalyco/rune) (`wgpu` / native `objc2-metal` backend).

- `crates/core` — UI/GPU-agnostic develop recipe, Fritsch-Carlson tone curves, 3-way color balance, 8-band HSL, session catalog & sidecar persistence.
- `crates/decode` — macOS `CIRAWFilter`/`ImageIO` + cross-platform `rawler` RAW decoding, fast filmstrip thumbnails, and `rayon` full-res export.
- `crates/engine` & `shaders/develop.wgsl` — single-pass linear `Rgba16Float` WGSL develop pipeline + `256x1` curve LUT texture.
- `crates/app` — Anomaly OpenCode-styled (`rune-theme-opencode`) & Apple macOS HIG-compliant darkroom UI (`Titlebar`/`ChromeInsets`, native `NSMenu`, 120Hz `Compositor`).

## Build & Storage Hygiene (IMPORTANT)
Rust + `wgpu` + `rune` build artifacts grow quickly (multi-GB `target/` directories and Cargo caches). Keep disk usage low:

1. **Clean up stale build artifacts regularly:**
   - Run `cargo clean` before switching between debug and release builds, after bumping pinned `rune` revisions, or whenever `target/` grows unnecessarily large (check with `du -sh target`).
   - Prefer keeping **only one** active build profile in `target/` (e.g. remove `target/debug` after building `--release`, or run `cargo clean -p rune-editor` / `cargo clean` between major milestones).
2. **Prune Cargo cache when needed:**
   - Periodically prune unused cached git checkouts and registry downloads (e.g. old `anomalyco/rune` revisions under `~/.cargo/git/checkouts` or `cargo cache -a` / removing superseded `target/` directories) to keep local storage down.
3. **Never commit build artifacts or temporary PNG snapshots** into the repository (`.gitignore` excludes `/target` and `*.png`). Save headless `--snap` verification images to your temporary/brain directory and clean them up when no longer needed.
4. **Track & report storage every turn (`STORAGE.md`):**
   - At the end of **every turn**, measure current usage (`du -sh . target target/* ~/.cargo/registry ~/.cargo/git` and `df -h .`), update `STORAGE.md` (Current Snapshot + a new Turn History row), and include a brief storage summary in your response to the user.

## Verification Checklist
- Run `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
- For UI changes, verify headless `--chrome --snap` renders across scenes (`default`, `color`, `tone-curve`, `split`, `filmstrip`) with `0 overlapping elements` on macOS traffic lights.
- Keep `shaders/develop.wgsl` and `crates/core` (`CompiledRecipe::evaluate_pixel_scene_linear` + `filmic_tonemap`) mathematically in parity.
