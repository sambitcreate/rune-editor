# AGENTS.md — Rune Darkroom (`rune-editor`)

> **Authoritative standards:** Read [docs/ENGINEERING_GUIDELINES.md](docs/ENGINEERING_GUIDELINES.md) before modifying recipes, shaders, exports, sidecars, CLI commands, or the MCP server.

## Project Overview
Mac-first, cross-platform-ready Capture One / Lightroom-style GPU-accelerated photo editor built in Rust on [Anomaly's Rune](https://github.com/anomalyco/rune) (`wgpu` / native `objc2-metal` backend).

- `crates/core` — UI/GPU-agnostic develop recipe, single-source parameter registry (`SCALAR_PARAMS`), typed `RecipePatch`, Fritsch-Carlson tone curves, 3-way color balance, 8-band HSL, perceptual analysis, session catalog & atomic sidecar persistence.
- `crates/decode` — macOS `CIRAWFilter`/`ImageIO` (`OnceLock` `CIContext`) + cross-platform `rawler` RAW decoding, `<1ms` header dimension probing, fast filmstrip thumbnails, and unified `rayon` full-res/preview export (with spatial detail).
- `crates/engine` & `shaders/develop.wgsl` — single-pass linear `Rgba16Float` WGSL develop pipeline + `256x1` curve LUT texture.
- `crates/mcp` — localhost-only MCP (`2025-11-25`) server over Streamable HTTP and stdio with live GUI viewport bridge.
- `crates/cli` — `rd` agent-native CLI (`inspect`, `analyze`, `edit`, `preview`, `diff`, `cull`, `export`, `batch`, `session`, `open`, `mcp`).
- `crates/app` — Anomaly OpenCode-styled (`rune-theme-opencode`) & Apple macOS HIG-compliant darkroom UI (`Titlebar`/`ChromeInsets`, native `NSMenu`, 120Hz `Compositor`).

## Build & Storage Hygiene (IMPORTANT)
Rust + `wgpu` + `rune` build artifacts grow quickly (multi-GB `target/` directories and Cargo caches). Keep disk usage low:

1. **Keep a single active build profile:**
   - Prefer `--release`-only local verification (`cargo test --workspace --release`, `cargo clippy --workspace --all-targets --release -- -D warnings`) when disk is tight so `target/debug` and `target/release` do not coexist.
   - Run `cargo clean` before switching profiles, after bumping pinned `rune` revisions, or whenever `target/` grows unnecessarily large (`du -sh target`).
2. **Prune Cargo cache when needed:**
   - Periodically prune unused cached git checkouts and registry downloads (e.g., old `anomalyco/rune` revisions under `~/.cargo/git/checkouts` or `cargo cache -a`) to keep local storage down.
3. **Never commit build artifacts or temporary PNG snapshots** into the repository (`.gitignore` excludes `/target` and `*.png`). Save headless `--snap` verification images to `/tmp` or your temporary/brain directory and clean them up when no longer needed.
4. **Track & report storage every turn (`STORAGE.md`):**
   - At the end of **every turn**, measure current usage (`du -sh . target target/* ~/.cargo/registry ~/.cargo/git` and `df -h .`), update `STORAGE.md` (Current Snapshot + a new Turn History row), and include a brief storage summary in your response to the user.

## Verification & Parity Checklist
See [docs/ENGINEERING_GUIDELINES.md](docs/ENGINEERING_GUIDELINES.md) for full details and security checklists.

- Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --release -- -D warnings`, and `cargo test --workspace --release`.
- For UI changes, verify headless `--chrome --snap` renders across scenes (`default`, `color`, `tone-curve`, `split`, `filmstrip`) with `0 overlapping elements` on macOS traffic lights.
- **GPU ↔ CPU Parity:** Keep `shaders/develop.wgsl` and `crates/core` + `crates/decode/src/export.rs` (including spatial detail, vignette, grain, and geometry) mathematically in parity within `< 2e-3`; route all CPU exports and MCP previews through `crates/decode/src/export.rs`.
- **GUI ↔ CLI ↔ MCP 1:1 Extensibility Rule:** Any new develop parameter, preset, geometry option, or tool added to `crates/core` / `crates/app` MUST be registered in `crates/core/src/registry.rs` (`SCALAR_PARAMS` + `registry_covers_all_recipe_fields`) and `RecipePatch` so it is immediately available in the GUI, `rd` CLI (`rd edit`, `rd schema`, `rd cli search`, `rd batch`), and MCP (`rune_*`), and documented in `skills/rune-darkroom/SKILL.md`.
- **Non-Destructive & MCP Security Rule:** Originals are inviolable (`guard_not_source`, RAW extension blocklist, atomic temp-dotfile + `sync_all` + `rename` writes, monotonic in-place `PhotoMeta.revision`, preserved `history`, auto-saved dirty session sidecars). Local MCP (`2025-11-25`) changes MUST preserve `127.0.0.1`-only bind, strict `Host`/`Origin` RFC 6454 + `u16` port validation, `set_nonblocking(false)` + Keep-Alive, bounded reads/concurrency, `/dev/urandom` sessions without an `is_empty()` bypass on `DELETE`, and safe `out` extensions with `overwrite: false` defaults.
