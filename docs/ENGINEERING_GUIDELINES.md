# Rune Darkroom — Engineering & Security Guidelines

This document is the authoritative engineering, color-pipeline, data-integrity, and security standard for **Rune Darkroom** (`rune-editor`). All pull requests and automated agent changes must adhere to these invariants.

---

## 1. GUI ↔ CLI ↔ MCP 1:1 Parity Contract

Rune Darkroom exposes the exact same develop engine across three surfaces: the 120Hz desktop GUI (`crates/app`), the `rd` CLI (`crates/cli`), and the local MCP server (`crates/mcp`). Parity is enforced by construction:

- **Single-source parameter registry (`crates/core/src/registry.rs`):**
  - Every top-level scalar develop slider MUST be declared in `SCALAR_PARAMS` with its canonical `id`, bounds (`min`/`max`), default, step, unit, group, and accessors.
  - Structural recipe fields (`shadows_wheel`, `midtones_wheel`, `highlights_wheel`, `curves`, `hsl`, `levels`, `bw`, `geometry`) MUST be registered in `registry.rs` (schema, dot-path `set`/`get`, group reset, and group copy — keep `reset_recipe_group` and `copy_recipe_groups` symmetrical for every group, including `tone` + `levels`).
  - The CI test `registry_covers_all_recipe_fields` fails the build if any field is added to `DevelopRecipe` without registering it in `registry.rs`.
  - GUI controls MUST read slider min/max bounds from `find_scalar_param(...)` instead of hardcoding duplicate limits (e.g., Exposure is `-5.0..=5.0` everywhere).
- **Shared mutation model (`DevelopRecipe` & `RecipePatch`):**
  - Typed sparse patches (`crates/core/src/patch.rs`) use `#[serde(deny_unknown_fields)]` so typos fail loudly across CLI and MCP.
  - Applying a preset (`apply_preset`) MUST preserve `recipe.geometry` (crop, straighten, 90° rotation, flip) identically across GUI, CLI, and MCP.
  - Keep `preset_name` semantics consistent: clear `preset_name` on manual/auto mutations and retain it only for full-strength (`mix == 1.0`) preset applications.
- **Single choke-point live GUI ↔ MCP sync:**
  - `AppState::sync_window_state` is the single choke point that calls `sync_mcp_canvas_snapshot()`, ensuring every GUI action (sliders, crop/geometry, ratings, cull flags, photo switches) immediately updates the shared MCP snapshot.
- **Agent skill documentation:**
  - Any new develop parameter, preset, geometry option, CLI subcommand, or MCP tool/argument MUST be reflected in [`skills/rune-darkroom/SKILL.md`](../skills/rune-darkroom/SKILL.md).

---

## 2. GPU ↔ CPU Color & Spatial Parity Invariant

What the photographer sees in the 120Hz Metal/`wgpu` viewport MUST match every CPU export, CLI preview, batch deliverable, and MCP inline preview.

- **Full-pipeline parity (`< 2e-3` tolerance):**
  - Every stage in `shaders/develop.wgsl` MUST have a mathematically equivalent CPU implementation in `crates/core` (`CompiledRecipe::evaluate_pixel_scene_linear` + `filmic_tonemap`) and `crates/decode/src/export.rs`:
    1. **Step 0 — Spatial detail:** bilateral chroma/luma `noise_reduction`, broad midtone `clarity`, fine micro-contrast `structure`, and edge-gated `sharpen` (with tap radii scaled proportionally when downscaling, and fast zero-cost bypass when all 4 are `0.0`).
    2. **Steps 1–10 — Point color science:** white balance, exposure, contrast, brightness, highlights/shadows/whites/blacks, tone curves (`256`-entry Fritsch-Carlson LUT), 3-way color balance wheels, 8-band HSL, levels, vibrance/saturation/dehaze, and B&W mixer.
    3. **Step 11 — Vignette** & **Step 12 — Film grain:** compute grain's midtone mask `t` from post-vignette `rgb` in both `develop.wgsl` and `export.rs`.
    4. **Geometry & resampling:** crop, straighten, 90° rotation, flip, and Catmull-Rom / high-quality resampling (never nearest-neighbor point sampling for previews).
- **Zero duplicated CPU render loops:**
  - GUI `Export…`, `rd export`, `rd preview`, `rd batch run`, and MCP `rune_preview_photo` (`export_image_to_jpeg_vec`) MUST all route through `crates/decode/src/export.rs`.
- **WGSL safety & automated guards:**
  - Never rely on lazy evaluation in WGSL `select(a, b, cond)` — guard denominators (e.g., `w.xy / max(len, 1e-6)` in `clamp_wheel`) and pass `.clamped()` wheels from `DevelopUniforms::build`.
  - Keep automated parity tests passing: `gpu_shader_compiles_and_matches_cpu_color_science` in `crates/engine` plus spatial-detail and JPEG-preview parity tests in `crates/decode`.

---

## 3. Non-Destructive Sidecar & Data Integrity Rules

Photographers' original RAW and raster files are inviolable.

- **Inviolable originals (`guard_not_source` + RAW blocklist):**
  - Source files are never modified in place. Every export and preview disk write MUST call the shared `rune_editor_decode::guard_not_source(src, dest)` (which canonicalizes existing parent directories even when `dest` does not exist yet or traverses symlinks) and reject any destination whose extension is in `RAW_EXTENSIONS`.
- **Collision-free batch exports:**
  - Multi-photo exports (`rd export`) MUST pre-compute all `(src, dest)` pairs before parallel execution and fail fast (`DUPLICATE_OUTPUT_PATH`) on stem collisions (e.g., `RAW+JPEG` pairs or `--recursive` subfolders with identical filenames).
- **Atomic writes (`temp dotfile` → `sync_all` → `rename`):**
  - All `.rune-edit.json` sidecar writes and rendered image exports MUST write to a sibling hidden temporary file, call `sync_all()`, and atomically `rename()` to the final path.
- **Monotonic `PhotoMeta.revision` & provenance `history`:**
  - Always advance in-memory `revision` in place on save (`save_sidecar_in_place`, `save_sidecar_checked_in_place`, `SessionCatalog::save_entry_sidecar_mut`).
  - Live MCP edits targeting a real file (`path` omitted while a disk photo is open in the GUI) MUST load existing `.rune-edit.json` metadata first so prior `revision` and `history` entries are preserved rather than wiped to `revision: 1`, and MUST resync GUI `saved_recipe` and `active_meta` so dirty state stays accurate.
  - Batch pipelines (`rd batch run`) MUST persist sidecars whenever either develop recipe fields OR photo metadata (`rating` / `flag`) changed.
- **Dirty session auto-save on photo switch:**
  - In folder sessions (`!demo_mode`), switching photos (`select_next_photo_tx`, `select_prev_photo_tx`, `select_catalog_index_tx`) MUST auto-save dirty `.rune-edit.json` sidecars before loading the next frame so edits are never silently lost across navigation or `⌘W`.
- **Symlink-safe directory traversal:**
  - Recursive folder scans (`crates/decode/src/folder.rs`) MUST skip directory symlinks (`file_type()?.is_symlink()`) and enforce `MAX_SCAN_DEPTH` (`32`) to prevent symlink-cycle stack overflow or OOM.

---

## 4. Local MCP (`2025-11-25`) Security Checklist

The embedded MCP server runs on the user's workstation and handles untrusted/prompt-injected tool inputs as well as local browser-reachable HTTP sockets. Every change to `crates/mcp` MUST satisfy all checks below:

- **Loopback-only binding:** Bind exclusively to `127.0.0.1` (never `0.0.0.0` or external interfaces).
- **Strict `Host` & `Origin` validation (anti-DNS-rebinding / RFC 6454):**
  - Require a `Host` header on HTTP/1.1 (`400 Bad Request` if missing).
  - Allow only `localhost`, `127.0.0.1`, or `[::1]` with an optional strictly parsed `u16` port (`port.parse::<u16>().is_ok() && !port.starts_with('+')`).
  - Reject `@`, `/`, `?`, `#`, whitespace, or control characters in `Host` and `Origin` (`403 Forbidden`) so payloads like `Origin: http://localhost:80@evil.example` or `Host: 127.0.0.1:anything` cannot bypass prefix checks.
- **macOS/BSD socket mode & HTTP/1.1 Keep-Alive:**
  - Accepted streams inherit `O_NONBLOCK` from a non-blocking `TcpListener` on macOS/BSD. Always call `stream.set_nonblocking(false)` and `stream.set_nodelay(true)` before entering the HTTP/1.1 Keep-Alive loop.
  - Use a short idle timeout (`10s`) between Keep-Alive requests and an active read/write timeout (`60s`) for request bodies and responses.
- **Bounded reads, strict framing & concurrency limits:**
  - Never use unbounded `BufRead::read_line`. Enforce caps: `8 KiB` per request line / header line / chunk-size line / trailer line, max `64` headers, max `16` trailers, max `4 MiB` HTTP body (`8 MiB` stdio line cap).
  - Cap concurrent HTTP connection threads at `16` via an atomic counter (return `503 Service Unavailable` when saturated).
  - Reject duplicate/conflicting `Content-Length`, simultaneous `Transfer-Encoding` + `Content-Length`, and invalid chunk trailing CRLF (`crlf != *b"\r\n"`) with `400 Bad Request`.
- **CSPRNG sessions & strict lifecycle:**
  - Generate `Mcp-Session-Id` from OS entropy (`/dev/urandom`, 16 random bytes hex) and store active sessions in a bounded LRU (`256` entries, evicting oldest only).
  - Track `sessions_issued` so `DELETE /mcp` removing the last active session never falls into an `is_empty()` bypass — any subsequent request using a deleted session ID MUST return `404 Not Found`.
  - Validate incoming `MCP-Protocol-Version` headers (`400 Bad Request` on unsupported versions) and verify `jsonrpc == "2.0"` (`-32600`).
- **Prompt-injection safe `out` paths:**
  - `rune_preview_photo` and `rune_cull_or_export` MUST restrict `out` to safe raster extensions (`.jpg`, `.jpeg`, `.png`, `.tif`, `.tiff`, `.webp`), reject `RAW_EXTENSIONS` and source-file paths via `guard_not_source`, and default to `overwrite: false` (refusing to overwrite existing files unless `overwrite: true` is explicitly provided).

---

## 5. Memory, Performance & Storage Hygiene

- **Bounded decode & prefetch pools:**
  - Keep GUI `prefetch_cache` capped at **2** full-resolution `LinearImage` entries with an in-flight deduplication set (`Arc<Mutex<HashSet<PathBuf>>>`) to prevent multi-GB RAM spikes during rapid navigation across 50–60MP RAW folders.
  - Use `<1ms` header-only `probe_image_dimensions(path)` (respecting EXIF orientation `5..=8`) whenever only image dimensions are required (e.g., aspect-preset cropping without full pixel decode).
- **macOS `OnceLock` `CIContext` & CPU fast paths:**
  - Reuse a single `OnceLock`-cached `CIContext` in `crates/decode/src/raw.rs` rather than allocating a fresh Metal context per decode.
  - Keep `CompiledRecipe` bypass flags (`tones_active`, `contrast_active`) and the 4096-entry linear-to-sRGB LUT intact for fast multi-core `rayon` exports.
- **Cross-platform boundaries:**
  - `crates/core` MUST remain strictly UI- and GPU-agnostic (zero `wgpu`, `rune`, or AppKit dependencies).
  - Guard Apple-only accelerations (`CIRAWFilter`, `ImageIO`, `NSMenu`) behind `#[cfg(target_os = "macos")]` and keep pure-Rust fallbacks (`rawler`, `image`) compiling and passing tests.
- **Build & disk storage hygiene:**
  - Rust + `wgpu` + `rune` target directories grow into multiple GB quickly. Keep **only one** active build profile in `target/` — prefer `--release`-only local verification when disk space is tight, and run `cargo clean` before switching profiles or after bumping the pinned `anomalyco/rune` revision.
  - Never commit build artifacts (`target/`) or temporary `--snap` PNGs (`*.png`). Periodically prune superseded `anomalyco/rune` checkouts under `~/.cargo/git/checkouts`.

---

## 6. PR Verification Checklist

Run and verify all checks before merging:

```bash
# 1. Formatting
cargo fmt --all --check

# 2. Zero-warning lints across all crates & targets (single release profile)
cargo clippy --workspace --all-targets --release -- -D warnings

# 3. Full workspace test suite (core registry parity, GPU↔CPU parity, decode, MCP security, CLI, app)
cargo test --workspace --release
```

For any UI, layout, theme, or shader change, also verify headless GPU renders across all scenes and confirm `0 overlapping elements` on macOS traffic lights (saving snapshots to `/tmp` or a scratch directory and removing them afterward):

```bash
for scene in default color tone-curve split filmstrip; do
  cargo run --release -p rune-editor -- --chrome --snap "/tmp/rune-${scene}.png" --scene "$scene"
done
rm -f /tmp/rune-{default,color,tone-curve,split,filmstrip}.png
```
