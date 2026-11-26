---
name: rune-darkroom
description: Non-destructively inspect, analyze, color-grade, cull, preview, verify, and batch-export RAW and raster photographs using the Rune Darkroom CLI (`rd`) and local MCP (`2025-11-25`) server. Use whenever the user asks to edit, color-grade, crop, cull, enhance, or batch-process photos or folders of images.
---

# Rune Darkroom CLI (`rd`) & Local MCP — Agent Workflow Skill

`rd` and the embedded **MCP (`2025-11-25`)** server provide fast, non-destructive, scene-referred linear photo editing sharing the exact develop engine (including spatial detail, vignette, grain, and geometry) and `.rune-edit.json` sidecar format as the **Rune Darkroom** macOS/cross-platform desktop app.

- **Non-destructive by default:** Originals are never modified; edits write atomically to `<photo>.rune-edit.json` with monotonic `revision` tracking, provenance `history`, and optimistic concurrency checks.
- **Agent-native output:** When stdout is piped, `rd` emits compact JSON / NDJSON automatically (pass `--json` or `--ndjson` explicitly if needed).
- **1:1 GUI ↔ CLI ↔ MCP extensibility:** Every slider, preset, and geometry control added to Rune Darkroom is registered in `crates/core/src/registry.rs`. Always run `rd schema recipe`, `rd cli search <term>`, or `rune_get_schema` when checking newly added tools—never guess parameter names.

---

## 1. Self-Discovery (Zero Hallucinated Flags)

```bash
rd capabilities                  # Check OS, RAW decode tier, formats, built-in presets
rd schema recipe                 # Exact parameter IDs, min/max bounds, defaults, enums
rd cli search skin               # Search commands, parameters, and presets by keyword
rd preset list                   # Built-in styles: neutral, warm_film, classic_chrome, editorial_bw, moody_dusk, alpine_crisp
```

---

## 2. Golden 5-Step Single-Photo Editing Loop

1. **Inspect & Quantitatively Analyze (1 call):**
   ```bash
   rd analyze photo.ARW --include exif
   ```
   Check `tone.clip_pct`, `tone.dynamic_range_stops_p01_p99`, `color.cast_lab_neutral`, `color.skin_tones`, `focus.sharpness_score`, and `suggested_auto`.
   Need to neutralize a gray card or white shirt at `(0.52, 0.48)`? Add `--sample 0.52,0.48`.

2. **Apply Edits Non-Destructively (`--dry-run` optional):**
   ```bash
   # Mix a built-in preset + Base Characteristics curve + WB preset + targeted scalar/HSL/detail nudges:
   rd edit photo.ARW \
     --preset classic_chrome --mix 0.8 \
     --film-curve film_extra_shadow \
     --wb-preset daylight \
     --set exposure+=0.25 \
     --set highlights=-0.35 \
     --set clarity_method=punch \
     --set sharpen_radius=1.4 \
     --set nr_color=0.25 \
     --set grain_type=silver_rich \
     --set grain_size=0.45 \
     --set hsl.orange.saturation=-0.12 \
     --agent antigravity
   ```
   Or pass a typed sparse JSON `RecipePatch` via stdin (`deny_unknown_fields` catches typos immediately):
   ```bash
   echo '{"film_curve":"film_high_contrast","wb_preset":"cloudy","exposure":"+0.30","highlights":-0.40,"curves":{"rgb":[[0,0],[0.25,0.22],[0.75,0.79],[1,1]]},"levels":{"red":{"in_black":0.02}},"wheels":{"master":{"hue_x":0.08,"luma":0.02}},"geometry":{"aspect_preset":"FourFive"}}' \
     | rd edit photo.ARW --patch -
   ```
   `RecipePatch` (shared by `rd edit --patch` and MCP `rune_edit_photo.patch`) supports:
   - **Base Characteristics, Film Negative Conversion & White Balance Presets:**
     - `processing_mode`: `"photography"` (default) or `"film_negative"` (CLI `--processing-mode <mode>`, MCP `rune_edit_photo.processing_mode`).
     - `film_negative`: Stage 0.5 linear orange-mask neutralization (`base_r`/`base_g`/`base_b` `0.05..=2.0`, `density` `-1..1`, `contrast` `-1..1`, `black_clip` `0..0.45`, `white_clip` `0.55..1.0`). Sample an unexposed film border with CLI `--pick-film-base <x,y>` or MCP `film_base_pick: [x, y]` (applied before `patch`/`set` to match CLI↔MCP precedence).
     - `film_curve`: `"film_standard"` (default), `"film_extra_shadow"`, `"film_high_contrast"`, `"linear_response"` (also CLI `--film-curve <curve>` and MCP `rune_edit_photo.film_curve`).
     - `wb_preset`: `"as_shot"` (default), `"daylight"`, `"cloudy"`, `"shade"`, `"tungsten"`, `"fluorescent"`, `"flash"`, `"custom"` (also CLI `--wb-preset <preset>` and MCP `rune_edit_photo.wb_preset`; `rd inspect` and `rune_inspect_photo` report the live `wb_kelvin` readout from `2000 K`–`14000 K`).
   - **Scalars:** bare number (`-0.40`), signed relative string (`"+0.30"`, `"-0.10"`, `"*1.2"`), or explicit object (`{"set": 0.35}`, `{"add": 0.25}`, `{"scale": 1.2}`).
     - Includes Phase 1 detail & grain controls: `clarity_method` (`"natural"`, `"punch"`, `"neutral"`, `"classic"`), `sharpen_radius` (`0.5..=4.0`, default `1.0`), `sharpen_threshold` (`0.0..=1.0`, default `0.0`), `sharpen_halo` (`0.0..=1.0`, default `0.0`), `nr_color` (`0.0..=1.0`, default `0.0`), `grain_type` (`"fine"`, `"silver_rich"`, `"soft"`, `"cubic"`, `"tabular"`, `"harsh"`), and `grain_size` (`0.0..=1.0`, default `0.35`).
   - **Color Balance (`wheels` or top-level `*_wheel` — 3-Way + Master):**
     - Grouped: `"wheels": { "shadows": { "hue_x": -0.10, "luma": -0.02 }, "midtones": { "hue_y": 0.04 }, "highlights": { "hue_x": 0.12 }, "master": { "hue_x": 0.06, "luma": 0.02 } }` (aliases `shadows_wheel`/`midtones_wheel`/`highlights_wheel`/`master_wheel` also accepted inside `wheels` and via dot-path `--set wheels.master_wheel.hue_x=0.06`).
     - Top-level: `"shadows_wheel": { ... }`, `"midtones_wheel": { ... }`, `"highlights_wheel": { ... }`, `"master_wheel": { ... }`.
   - **Per-Channel Levels (`levels` — Master RGB + `red`/`green`/`blue`):**
     - Master RGB: `levels.{in_black,in_mid,in_white,out_black,out_white}`.
     - Per-channel: `levels.{red,green,blue}.{in_black,in_mid,in_white,out_black,out_white}` (e.g. `"levels": { "red": { "in_black": 0.02 }, "blue": { "out_white": 0.96 } }`).
   - **Tone Curves (`curves.{rgb,luma,red,green,blue}` — all 4 control-point formats plus `"reset"`/`"identity"`):**
     1. **Pair array:** `[[0, 0], [0.25, 0.20], [0.75, 0.80], [1, 1]]`
     2. **Flat array** (even count `>= 4`): `[0, 0, 0.25, 0.20, 0.75, 0.80, 1, 1]`
     3. **Keyed map:** `{"0": 0, "0.25": 0.20, "0.75": 0.80, "1": 1}`
     4. **Semicolon/comma string:** `"0,0;0.25,0.20;0.75,0.80;1,1"` (or `"reset"`)
   - **Black & White + Split Tones (`bw`):**
     - `bw.enabled`, 6-channel color sensitivity `bw.{red,yellow,green,cyan,blue,magenta}` (`-1..1`), and split toning `bw.split_shadow_hue` (`0..360`), `bw.split_shadow_sat` (`0..1`), `bw.split_highlight_hue` (`0..360`), `bw.split_highlight_sat` (`0..1`).
   - **Advanced Color Editor (4 custom slices `advanced_color.{0..3}`):**
     - Each slice supports `enabled`, `target_hue` (`0..360`), `target_sat` (`0..1`), `hue_range` (`5..180`), `sat_range` (`0.05..1`), `smoothness` (`0..1`), `hue_shift` (`-1..1`), `saturation` (`-1..1`), `lightness` (`-1..1`), and `uniformity` (`0..1`).
     - Eyedropper pickers: CLI `--pick-color-slice <idx:x,y>` (repeatable, e.g. `--pick-color-slice 0:0.25,0.20`), MCP `color_slice_pick: {"index": 0, "x": 0.25, "y": 0.20}` (applied before `patch`/`set`).
   - **Skin Tone Uniformity (`skin_tone`):**
     - `skin_tone.enabled`, `target_hue` (`0..360`), `target_sat` (`0..1`), `target_lightness` (`0..1`), `smoothness` (`0..1`), `uniformity_hue`/`uniformity_sat`/`uniformity_lightness` (`0..1`), and `amount_hue`/`amount_sat`/`amount_lightness` (`-1..1`).
     - Eyedropper picker: CLI `--pick-skin <x,y>` (e.g. `--pick-skin 0.52,0.40`), MCP `skin_pick: [0.52, 0.40]` (samples linear pixel and enables `skin_tone` before `patch`/`set`).
   - **Crop-Aware Vignetting (`vignette_method`), Optical Lens Correction (`lens`), & Keystone Perspective (`geometry.keystone_*`):**
     - `vignette_method`: `"elliptic_on_crop"` (default), `"circular_on_crop"`, `"circular"` (also CLI `--vignette-method <slug>` and MCP `vignette_method`).
     - `lens`: `lens.distortion` (`-1..1`), `lens.chromatic_aberration` (`-1..1`), `lens.purple_fringing` (`0..1`), `lens.light_falloff` (`0..1`).
     - `geometry` keystone: `geometry.keystone_vertical` (`-1..1`), `geometry.keystone_horizontal` (`-1..1`), `geometry.keystone_aspect` (`-0.5..1.0`), `geometry.keystone_amount` (`0..1`, default `0.8`).

3. **Render a Visual Proof & Inspect With Vision:**
   ```bash
   rd preview photo.ARW -o /tmp/check.jpg --max-edge 1280 --quality 85
   ```
   Open `/tmp/check.jpg` with your image-reading tool to verify skin tones, highlight roll-off, composition, and mood.

4. **Verify Perceptual Delta & Guardrails (`CIEDE2000`):**
   ```bash
   rd diff photo.ARW --assert max_highlight_clip_pct=0.5,max_skin_de2000=8.0
   ```
   Exits `0` if all guardrails pass, `1` if any limit is violated.

5. **Export Full-Resolution, Watermarked, Display P3, or Multi-Recipe Deliverables (or Hand Off to GUI):**
   ```bash
   # Single watermarked Display P3 JPEG with dynamic naming tokens:
   rd export photo.ARW -o ./deliverables --to jpeg --quality 92 \
     --naming-template '{ImageName}_{Recipe}_{ISO}' \
     --watermark '© Studio' --watermark-opacity 0.45 --watermark-position bottom_right \
     --color-space display-p3 --strip-metadata --output-sharpen 0.25

   # Simultaneous Multi-Recipe export (Full-Res JPEG + Web 2048px + Archival 16-bit TIFF):
   rd export ./shoot --picks -o ./deliverables --recipes full_res_jpeg,web_2048px,archival_16_bit_tiff

   # Or hand off to the human in Rune Darkroom GUI:
   rd open photo.ARW
   ```

---

## 3. Batch, Virtual Variants, User Styles & Selective Clipboard Workflows

### A. Fast Folder Scan, 4-Folder Sessions, Hot Folder Auto-Ingest, 7 Color Tags & Culling
```bash
# Initialize a Capture One 4-folder session (Capture/, Selects/, Output/, Trash/):
rd session init ./client-shoot

# Watch a Hot Folder for newly arriving frames & auto-apply a Style (`--once` for deterministic scripts):
rd session watch ./client-shoot/Capture --style warm_film --once
rd session watch ./client-shoot/Capture --style "Editorial Warm" --interval-ms 500

# Convert a camera-scanned film negative by sampling its unexposed orange border:
rd edit ./scans/frame01.DNG --pick-film-base 0.04,0.04 --set film_negative.density=0.15 --set film_negative.contrast=0.20

# Scan & filter folder metadata by Pick, Rating, Color Tag, and text search:
rd session scan ./shoot --recursive --color-tag purple --search sony

# Stream perceptual + sharpness metrics for every photo (NDJSON):
rd analyze ./shoot --ndjson > /tmp/shoot.ndjson

# Reject soft frames via stdin pipe:
jq -r 'select(.analysis.focus.sharpness_score < 20) | .path' /tmp/shoot.ndjson \
  | rd cull --from-stdin --reject

# Rate, pick, and assign Capture One Color Tags (none|red|orange|yellow|green|blue|purple|pink):
rd cull ./shoot/DSC0012.ARW ./shoot/DSC0015.ARW --pick --rate 5 --color-tag green
```

### B. Virtual Variants, Reusable User Styles & Selective Adjustments Clipboard
```bash
# Non-destructive Virtual Variants inside `.rune-edit.json`:
rd variant list photo.ARW
rd variant clone photo.ARW --label "B&W Print"
rd variant new photo.ARW --label "Clean Base"
rd variant select photo.ARW --index 1
rd variant delete photo.ARW --index 2

# Save, stack-apply, and manage reusable User Styles (~/.config/rune-darkroom/styles):
rd style save photo.ARW --name "Editorial Warm" --autoselect-adjusted
rd style list
rd style apply "Editorial Warm" ./shoot/DSC0015.ARW --stack --strength 0.65
rd style delete "Editorial Warm"

# Selective Adjustments Clipboard (`--groups` or Capture One `--autoselect-adjusted`):
rd copy-look ./shoot/DSC0012.ARW ./shoot/DSC0015.ARW --groups wb,tone,color,curves
rd copy-look ./shoot/DSC0012.ARW ./shoot --autoselect-adjusted
```

### C. Selective Copy/Paste Across a Burst (via `rd edit`)
```bash
rd edit ./shoot --picks --copy-from ./shoot/DSC0012.ARW --only wb,tone,color,hsl,curves
```

### D. Adjustment Layers, Combine Masks, AI Subject/Background & Heal/Clone (`rd layer`, `rd heal`)
```bash
# Add a sky-burn linear gradient layer, subtract a radial subject halo, and intersect bright highlights:
rd layer add photo.ARW --name "Sky Burn" --kind linear --opacity 0.85 --set exposure=-0.6 --set dehaze=0.25
rd layer mask-add photo.ARW --index 0 --kind radial --op subtract --center 0.5,0.5 --radius 0.25,0.20 --feather 0.45
rd layer mask-add photo.ARW --index 0 --kind luma --op intersect --low 0.45 --high 1.0 --low-falloff 0.12

# One-click AI Subject & AI Background layers:
rd layer add photo.ARW --name "Subject Pop" --kind ai-subject --set exposure=0.35 --set clarity=0.20
rd layer add photo.ARW --name "Bg Dim" --kind ai-background --opacity 0.75 --set exposure=-0.40

# Inspect, update, clear masks, or remove layers:
rd layer list photo.ARW
rd layer update photo.ARW --index 0 --opacity 0.9 --set exposure+=0.1
rd layer mask-clear photo.ARW --index 0
rd layer remove photo.ARW --index 2

# Non-destructive Spot Heal (auto-selected source patch) & Clone stamps:
rd heal add photo.ARW --target 0.42,0.38 --radius 0.03 --mode heal
rd heal add photo.ARW --target 0.70,0.60 --source 0.60,0.60 --radius 0.04 --mode clone
rd heal list photo.ARW
rd heal remove photo.ARW --index 0
rd heal clear photo.ARW
```

### E. Declarative Batch Manifest (`rd batch run`)
Use a JSON manifest for conditional ISO/clipping rules and multi-target exports (always test with `--dry-run` first):
```json
{
  "schema_version": 1,
  "agent": "antigravity",
  "inputs": { "paths": ["./shoot"], "picks_only": true, "min_rating": 3 },
  "pipeline": [
    { "name": "Base Look", "ops": [{ "op": "apply_preset", "id": "warm_film", "mix": 0.75 }] },
    { "name": "High-ISO Denoise", "when": { "iso_gte": 1600 }, "ops": [{ "op": "patch", "noise_reduction": 0.32, "sharpen": 0.18 }] },
    { "name": "Highlight Guard", "when": { "highlight_clip_pct_gt": 1.0 }, "ops": [{ "op": "patch", "highlights": "-0.25" }] }
  ],
  "sidecar_mode": "update",
  "exports": [
    { "id": "archive", "output_dir": "./out/archive", "format": "tiff16" },
    { "id": "web", "output_dir": "./out/web", "filename_template": "{stem}_web.{ext}", "format": "jpeg", "quality": 90, "max_edge": 2048, "output_sharpen": 0.25 }
  ]
}
```
Run with:
```bash
rd batch run job.json --dry-run
rd batch run job.json --jobs 2 --ndjson
```

---

## 4. Photographic Color-Grading Heuristics

- **Highlight Clipping (`clip_pct.highlight_any > 1.0%`):** Pull `exposure` (`-0.2..-0.5`) and `highlights` (`-0.3..-0.6`); lift midtones with `brightness` (`+0.1..+0.2`) instead of pushing `whites`.
- **High-ISO Frames (`exif.iso >= 1600`):** Set `noise_reduction` (`0.20..0.45`) and `nr_color` (`0.25..0.55`) to suppress chroma speckle, keep `sharpen <= 0.25` with `sharpen_threshold >= 0.15` and `sharpen_halo >= 0.20`, and avoid pushing `shadows > 0.35`.
- **Portraits & Skin Tones:** Prefer `film_curve: "film_extra_shadow"` for gentle shadow roll-off, `vibrance`, targeted `hsl.orange.*`, or Capture One-style **Skin Tone Uniformity** (`--pick-skin <x,y>` + `skin_tone.uniformity_hue=0.45`, `skin_tone.uniformity_sat=0.25`) to even out blotchy redness without plastic blur. Keep `clarity <= 0.15` with `clarity_method: "natural"`. If `skin_tones.note == "magenta_cast"`, nudge `tint` slightly negative (`-0.05`) or `hsl.orange.hue` positive (`+0.06`).
- **Architecture & Wide-Angle Optics:** Straighten converging verticals with `geometry.keystone_vertical` (`+0.15..+0.45`, default `keystone_amount=0.80`), correct barrel distortion (`lens.distortion`), clean high-contrast purple edges (`lens.purple_fringing=0.35..0.65`), and lift lens corner shading (`lens.light_falloff`). Pair creative vignettes with `vignette_method: "circular_on_crop"` or `"elliptic_on_crop"` so corner darkening follows the crop frame.
- **Editorial Contrast & B&W Split Toning:** Pair `film_curve: "film_high_contrast"` or gentle negative `blacks` (`-0.08..-0.18`) with positive `contrast` (`+0.12..+0.25`) and subtle 3-way/master wheel toning (`shadows_wheel.hue_x=-0.10`, `highlights_wheel.hue_x=+0.12`). For selenium/warm-paper monochrome, set `bw.enabled=true`, `bw.split_shadow_hue=220`, `bw.split_shadow_sat=0.18`, `bw.split_highlight_hue=42`, `bw.split_highlight_sat=0.14`, and `grain_type=silver_rich`.

---

## 5. Local MCP Server (`2025-11-25`) — Live GUI & Stdio Integration

Rune Darkroom ships a localhost-only (`127.0.0.1`) **MCP `2025-11-25`** server with 9 annotated tools:

- **Live GUI Streamable HTTP (`http://127.0.0.1:37420/mcp`):** Enable **Settings (`⌘,`) → Agent & Local MCP → Local MCP Server** inside Rune Darkroom. Omit `path` on any tool to inspect, preview, diff, or edit the photo currently open in the 120Hz Metal viewport (preserving existing `.rune-edit.json` `revision` + `history` and supporting `⌘Z` undo).
- **Stdio CLI (`rd mcp`):** Runs headlessly over stdin/stdout (or `rd mcp --http --port 37420` for standalone localhost HTTP).

### MCP Tool Highlights & Safety Contracts

| Tool | Key Parameters & Capabilities |
| :--- | :--- |
| `rune_get_schema` | Returns registered parameter bounds, tool groups, built-in presets, and the active GUI session snapshot. |
| `rune_inspect_photo` | Fast read-only EXIF, Kelvin WB readout (`wb_kelvin`, `wb_preset`), `film_curve`, **`processing_mode`** (+ `film_negative` when active), rating, cull flag, **`color_tag`**, virtual variant summary, **`layer_count`**, **`layers`**, **`heal_spot_count`**, and non-default recipe overrides (`path` optional). |
| `rune_analyze_photo` | Quantitative tone, clipping, Zone 0–X, Lab cast, skin tones, palette, sharpness, and auto suggestions (`path`, `proxy_edge`, `fast`). |
| `rune_edit_photo` | Non-destructive edit with `preset` + `mix`, `auto` (`["wb","tone","levels","all"]`), **`processing_mode`** (`photography`/`film_negative`), **`film_base_pick`** (`[x, y]`), **`wb_pick`** (`[x, y]`), **`wb_preset`**, **`film_curve`**, **`vignette_method`**, **`skin_pick`** (`[x, y]`), **`color_slice_pick`** (`{index, x, y}`), typed sparse **`patch`** (supports `processing_mode`, `film_negative.*`, `curves.rgb`, per-channel `levels.{red,green,blue}`, `wheels.master`, `bw.split_*`, `clarity_method`, `sharpen_radius`/`threshold`/`halo`, `nr_color`, `grain_type`, `grain_size`, `advanced_color`, `skin_tone`, `lens`, `geometry.keystone_*`), dot-path `set`, `reset`, `agent`, and `dry_run`. |
| `rune_preview_photo` | Full-parity render (spatial detail, vignette, grain, geometry, layers, heal/clone, film negative, Catmull-Rom) returning inline base64 JPEG `ImageContent` + clipping telemetry. Optional `out` writes to disk safely (raster extensions only; rejects RAW/source paths; `overwrite: false` by default — pass `"overwrite": true` to replace an existing file). |
| `rune_diff_photo` | Computes CIEDE2000 color delta, EV shift, clipping deltas, and field diffs (`against_preset` optional). Supports **`assertions`** (`max_highlight_clip_pct`, `max_shadow_clip_pct`, `max_skin_de2000`, `max_mean_de2000`) to enforce quantitative guardrails. |
| `rune_cull_or_export` | `action: "cull"` (`rating` `0..5`, `flag`: `pick`/`reject`/`none`, **`color_tag`**: `none`/`red`/`orange`/`yellow`/`green`/`blue`/`purple`/`pink`) or `action: "export"` (`out`, `format`, `quality`, `max_edge`, **`naming_template`**, **`watermark`** `{text, opacity, scale, position}`, **`strip_metadata`**, **`color_space`** `srgb`/`display_p3`, **`recipes`** multi-recipe array, **`overwrite`** defaulting to `false`). Rejects RAW extensions, non-image extensions, traversal subfolders, and source-file paths. |
| `rune_manage_workflow` | Capture One workflow operations (`action`): **`variant_list`**, **`variant_new`**, **`variant_clone`**, **`variant_select`**, **`variant_delete`**, **`style_list`**, **`style_save`** (`autoselect_adjusted`, traversal-safe slug), **`style_apply`** (`stack`, `strength`), **`style_delete`**, **`copy_selective_groups`** (`src_path`, `dst_path`, `groups`, `autoselect_adjusted`), and **`session_init`** (`dir`, traversal/symlink-safe 4-folder Capture One session creation). |
| `rune_edit_layers` | Non-destructive Adjustment Layers (up to 8), Composable Masks (`add`/`subtract`/`intersect` for `filled`/`linear`/`radial`/`luma`/`brush`/`ai_subject`/`ai_background`), Local Adjustments, and Spot Heal/Clone (up to 16) via `action`: **`layer_list`**, **`layer_add`**, **`layer_update`**, **`layer_remove`**, **`mask_add`**, **`mask_clear`**, **`ai_subject`**, **`ai_background`**, **`heal_list`**, **`heal_add`**, **`heal_remove`**, **`heal_clear`** (live GUI `⌘Z` undo sync). |

### Example MCP Tool Calls

```json
// 1. Live-GUI edit with film_curve, wb_preset, skin_pick, color_slice_pick, lens, and keystone:
{
  "film_curve": "film_extra_shadow",
  "wb_preset": "daylight",
  "vignette_method": "circular_on_crop",
  "skin_pick": [0.52, 0.40],
  "color_slice_pick": { "index": 0, "x": 0.20, "y": 0.15 },
  "patch": {
    "exposure": "+0.25",
    "highlights": -0.35,
    "vignette": -0.20,
    "clarity": 0.18,
    "clarity_method": "natural",
    "sharpen": 0.30,
    "sharpen_radius": 1.4,
    "sharpen_threshold": 0.10,
    "sharpen_halo": 0.25,
    "nr_color": 0.20,
    "grain": 0.15,
    "grain_type": "silver_rich",
    "grain_size": 0.45,
    "skin_tone": { "uniformity_hue": 0.45, "uniformity_sat": 0.25 },
    "advanced_color": { "0": { "uniformity": 0.35, "saturation": 0.18 } },
    "lens": { "distortion": -0.12, "chromatic_aberration": 0.20, "purple_fringing": 0.35 },
    "levels": { "red": { "in_black": 0.02 } },
    "wheels": {
      "master": { "hue_x": 0.04, "luma": 0.01 },
      "shadows": { "hue_x": -0.10, "luma": -0.02 },
      "highlights": { "hue_x": 0.12 }
    },
    "curves": {
      "rgb": [[0, 0], [0.25, 0.22], [0.75, 0.79], [1, 1]],
      "luma": [[0, 0], [0.25, 0.20], [0.75, 0.80], [1, 1]]
    },
    "geometry": { "aspect_preset": "FourFive", "keystone_vertical": 0.18 }
  }
}

// 2. Verify perceptual guardrails in rune_diff_photo:
{
  "against_preset": "neutral",
  "assertions": {
    "max_highlight_clip_pct": 0.5,
    "max_shadow_clip_pct": 1.0,
    "max_skin_de2000": 8.0,
    "max_mean_de2000": 15.0
  }
}

// 3. Safe disk preview / export (explicit overwrite required if file already exists):
{
  "out": "/tmp/proof.jpg",
  "max_edge": 1280,
  "quality": 85,
  "overwrite": false
}
```

---

## 6. Exit Codes

| Code | Meaning |
| :--- | :--- |
| `0` | Success (all items ok or skipped cleanly) |
| `1` | Partial batch failure (check NDJSON `error` events) or `rd diff --assert` violation |
| `2` | Invalid flag, unknown parameter/preset, duplicate export output path, or out-of-range value |
| `3` | Input file/folder not found or unsupported format |
| `4` | RAW/image decode failure |
| `5` | Export/sidecar I/O error or concurrent `SIDECAR_CONFLICT` (pass `--force` to override) |
| `6` | GPU backend unavailable when `--backend gpu` was explicitly requested |
