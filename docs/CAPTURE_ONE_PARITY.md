# Capture One Pro (16.9.1) vs. Rune Darkroom — Exhaustive Feature Parity Matrix

> **Last updated:** November 26, 2026  
> **Reference baseline:** Capture One Pro / Studio / Enterprise / Mobile **16.9.1** (verified against all 10 categories, 84 sections, and 1,299 published articles on `support.captureone.com` plus official 16.2–16.9.1 release notes).  
> **Rune Darkroom baseline:** `main` (`0.1.0`, Rust + Anomaly Rune + single-pass `Rgba16Float` WGSL / Metal backend + `rd` CLI + Local MCP `2025-11-25`).

---

## Status Legend

| Badge | Meaning |
| :---: | :--- |
| ✅ | **Shipped** — Implemented and verified in Rune Darkroom |
| 🟡 | **Partial** — Core capability exists; specific Capture One sub-modes/options still missing |
| ⬜ | **Not Yet Implemented** — Planned / open for PRs |
| ➖ | **Out of Scope / Niche Enterprise** — Hardware/cloud/enterprise-only unless community-driven |

---

## Executive Scorecard

| # | Domain | Rune Darkroom Status | Key Shipped Strengths | Biggest Next Gaps |
| :-: | :--- | :---: | :--- | :--- |
| 1 | Base Characteristics & RAW Engine | 🟡 | macOS `CIRAWFilter` + `rawler` linear FP16 (**738-camera dual-illuminant Bradford matrix calibration**), **14 MIT Camera Profiles** (`Rune ProStandard` OKLab hue-invariant + 9 Film Simulations) + pure-Rust **`.dcp`/`.icc`/`.icm`/`.cube` import w/ EXIF auto-match** (`33³` GPU↔CPU 3D LUT), **Base Characteristics Film Curves** (`Standard`/`Extra Shadow`/`High Contrast`/`Linear`), **Film Negative Conversion (`ProcessingMode` + Base Picker)** | Proprietary manufacturer `.dcp`/`.icc` profiles (user-imported, not redistributed), custom X-Trans/GPU demosaic |
| 2 | White Balance & Calibration | 🟡 | **Kelvin readout (`2,000–14,000 K`)**, **8 WB Presets**, Temp/Tint, WB Eyedropper (`3×3`), Auto WB (`A`) | Normalize, Smart Adjustments, Match Look |
| 3 | Exposure, HDR & Tone | ✅ | Exposure (`±5 EV`), Contrast, **Brightness**, HDR H/S/W/B, Auto Tone (`A`), **Exposure/Shadow Clipping Overlay (`⌘E`)** | Configurable Auto checklist, RAW single-channel highlight reconstruction |
| 4 | Levels & Tone Curves | 🟡 | **Master + Per-Channel R/G/B Levels** (+ Auto 0.1% clip + histogram), **5-Tab Fritsch-Carlson RGB/Luma/R/G/B Curves** (`256×1` LUT) + **Live Hover Tonal Indicator** | Curve & Levels eyedroppers, Target Levels |
| 5 | Color Balance, Color Editor & B&W | 🟡 | **3-Way + Master Color Wheels**, **3-Tab Color Editor** (8-Band Basic + **4-Slice Advanced w/ Uniformity & View Selected Color Range** + **Skin Tone Uniformity**), **6-Channel B&W + Split Tones** | Direct Cursor Tool (`D`), unlimited slices & 2D wireframe wheel, Masked Layer export |
| 6 | Clarity, Dehaze & Vignetting | 🟡 | Spatial Clarity + **4 Clarity Methods** (`Natural`/`Punch`/`Neutral`/`Classic`), **Structure**, **Dehaze**, **Crop-Aware Vignetting** (`Elliptic Crop`/`Circular Crop`/`Circular`) | Dehaze Auto / shadow tone picker |
| 7 | Detail, Noise, Grain & Artifacts | 🟡 | **Full 4-Param Sharpening** (`Amount`/`Radius`/`Threshold`/`Halo`), **Luminance + Color NR**, **6 Film Grain Types + Granularity**, **Purple Fringing** | AI Enhanced Denoise, Moiré, Dust Removal |
| 8 | Lens Correction & LCC | 🟡 | **Optical Lens Correction** (`Distortion`, `Chromatic Aberration`, `Light Falloff`) | Lensfun/Manufacturer profiles, Auto CA Analyze, Diffraction, LCC flat-field |
| 9 | Composition & Geometry | 🟡 | **Interactive Crop** (8 aspect presets incl. `4:5`, drag handles), **Straighten (`±45°`)**, **90° Rotate / Flip H/V**, **Composition Grids**, **Keystone Perspective** (`Vertical`/`Horizontal`/`Aspect`/`Amount`), 120Hz Zoom/Pan | 4-point on-canvas Keystone guides + Auto Keystone, AI Crop, Auto Rotate, Custom Ratios |
| 10 | Multi-Image Computational | ⬜ | — | HDR Merge (linear DNG), Panorama Stitch, Focus Stacking |
| 11 | Layers, Masks & Local Adjustments | 🟡 | **Adjustment Layers (up to 8)**, **Combine Masks (`Add`/`Subtract`/`Intersect`)**, **Linear (`L`), Radial (`T`), Luma Range, Brush (`B`)/Eraser (`E`)**, **Rubylith (`M`)/Grayscale (`⌥M`) Mask View**, **Spot Heal (`Q`) & Clone (`S`)** | Up to 16 layers, Magic Brush, Auto Mask edge-aware brush, multi-stroke Heal layers, Style Brushes |
| 12 | AI Masking & Portrait Retouching | 🟡 | **One-Click Subject & Background Masking** (`256×256` offline Lab saliency + joint-bilateral edge refinement) | Apple Neural Engine (`objc2-vision`) Subject/People sub-masks, AI Select/Eraser, Face/Eye/Teeth/Hair Retouch |
| 13 | Sessions, Catalogs, Variants & Stacks | 🟡 | **4-Folder Capture One Sessions** (`Capture/Selects/Output/Trash`, `⌘J`), `.rune-edit.json` sidecars, **Virtual Variants (`New`/`Clone`/`Select`/`Delete`)**, **14-Group Selective Adjustments Clipboard (`Autoselect Adjusted`)** | Session Albums/Templates, Variant Stacks & Multi-Edit, SQLite Catalogs |
| 14 | Culling, Ratings, Filters & Search | 🟡 | Filmstrip + **Grid Browser (`G`)**, 0–5★ ratings, Pick/Reject + **7 Color Tags** + **Auto-Advance**, **Compare Variant (`⌘⏎` orange border)**, **Color Tag & Text Search filters**, **Focus Mask (`⌘M`)** | Group Similar, AI Assisted Review (closed eyes), Secondary Display |
| 15 | Metadata, Keywords & Naming | 🟡 | **macOS ImageIO EXIF telemetry** in status bar, **Dynamic Naming Tokens** (`{ImageName}`, `{Counter}`, `{Date}`, `{Camera}`, `{Lens}`, `{ISO}`, `{Variant}`, `{Recipe}`) | IPTC editor, XMP sidecar sync, Hierarchical Keywords, Batch Rename |
| 16 | Styles, Presets, Speed Edit & Automation | 🟡 | 6 Built-in Styles, **User Styles + Additive Style Stacking**, **Speed Edit**, **Custom Keyboard Shortcuts UI**, native macOS menus, **`rd` CLI + Local MCP (`2025-11-25`, 9 tools)** | Style Brushes, `.costyle` import, Custom Workspaces |
| 17 | Viewer, Scopes & Proofing | 🟡 | Live RGB+Luma Histogram, **Live + 4 Pinned RGB/HSL/Lab Color Readout Markers**, **120Hz GPU Loupe (`O` / `⌥L`)**, **Proof Margin (`Compact`/`Studio`/`Exhibition`, `⌘⇧M`)**, **Clipping Warning**, **Focus Mask**, Before/After Split, 120Hz Metal | ICC Soft-Proofing, Wide-Gamut Display P3 swapchain & XDR/EDR |
| 18 | Export, Recipes, External Edit & Print | 🟡 | `rayon` parallel **JPEG/PNG-8/PNG-16/TIFF-16/WebP** export, **Simultaneous Multi-Recipe Queue**, **Dynamic Naming Tokens**, **Text Watermarks**, **Metadata Strip**, **Display P3 matrix**, **`Edit With…` 16-bit TIFF handoff**, Crop/Rotate/Flip/Resize/Output Sharpen | Image Watermarks, full ICC profile embedding (`moxcms`), AVIF/JXL, Contact Sheets, Print |
| 19 | Tethering, Live View, Collaboration & Studio | 🟡 / ➖ | **Hot Folder Auto-Ingest (`HotFolderWatcher`, GUI toggle + `rd session watch`, optional auto-Style)** | USB/Wi-Fi Camera Tethering, Live View, Overlay, LAN/Web Client Review |

> **Where Rune Darkroom Aims to Leapfrog Capture One:**
> - **100% Open Source (MIT)** in Rust—no subscription lock-in, full contributor extensibility.
> - **Agent-Native 1:1 CLI (`rd`) & Local MCP (`2025-11-25`)** — Single-source parameter registry (`SCALAR_PARAMS`) powering the 120Hz desktop GUI, `rd` CLI (auto-JSON/NDJSON, CIEDE2000 `diff --assert`, batch manifests), and localhost Streamable HTTP + stdio MCP server with live GUI `⌘Z` undo sync.
> - **True Extended Dynamic Range (XDR/EDR) & ISO 21496-1 Gain Map HDR** *(Future Roadmap)* — Capture One still lacks native EDR peak-brightness display rendering and HDR Gain Map export as of 16.9.1.
> - **Ultra-Fast Modern Formats:** Native **WebP** export already shipped; planned **JPEG XL & AVIF** export (none of which Capture One exports as of 16.9.1).
> - **Headless GPU Visual CI (`--chrome --snap`)** + Anomaly OpenCode aesthetic + 120Hz single-pass Metal/wgpu architecture.

---

## Phased Parity Tracker (Phases 1–5)

| Phase | Milestone Focus | Status | Key Deliverables | Eng Sign-off | UX Sign-off | Workspace Tests |
| :---: | :--- | :---: | :--- | :---: | :---: | :---: |
| **Phase 1** | **Color, Tone & Detail Depth** | ✅ Complete | Base Characteristics `FilmCurve` (4/6), Kelvin WB + `WbPreset` (8), 5-tab `RGB` Tone Curve + live tonal cursor indicator, Per-Channel R/G/B Levels, Master Color Wheel, B&W Split Tones, Clarity Methods (4), Sharpen Radius/Threshold/Halo, Color NR, 6 Film Grain profiles + Granularity — 1:1 GUI + `rd` CLI + MCP | ✅ `rev-eng` (#86) | ✅ `rev-ux` (14/14 scenes, #85) | 77/77 passed |
| **Phase 2** | **Advanced Color, Skin Tone, Vignette, Optics & Keystone** | ✅ Complete | 3-Tab Color Editor (Basic + 4-slice Advanced w/ Uniformity & `View Selected Color Range` + Skin Tone Uniformity), Crop-Aware Vignette (`Elliptic Crop`/`Circular Crop`/`Circular`), Optical Lens Correction (`Distortion`, `CA`, `Purple Fringing`, `Light Falloff`), 4-param Keystone Perspective — 1:1 GUI + `rd` CLI + MCP | ✅ `rev-eng` (#100) | ✅ `rev-ux` (16/16 scenes, #96) | 85/85 passed |
| **Phase 3** | **Pro Workflow: Variants, Color Tags, Styles, Loupe & Multi-Recipe Export** | ✅ Complete | 7 Color Tags + Auto-Advance, Grid Browser + Faceted Filters, Virtual Variants + Compare Variant (orange border), Selective Adjustments Clipboard (`Autoselect Adjusted`), User & Stacked Styles, 120Hz GPU Loupe + 4 Pinned Color Readout Markers, Dynamic Naming Tokens, Text Watermarks, Display P3 Export, Multi-Recipe Export Queue — 1:1 GUI + `rd` CLI + MCP | ✅ `rev-eng` (#115) | ✅ `rev-ux` (12/12 scenes, #114) | 93/93 passed |
| **Phase 4** | **Layers, Local Masks, Heal/Clone & Offline Subject AI** | ✅ Complete | Adjustment Layers (up to 8), Combine Masks (`Add`/`Subtract`/`Intersect`), Linear/Radial SDF & Luma Range masks, Brush/Eraser, Rubylith (`M`) & Grayscale (`⌥M`) mask display, Spot Heal & Clone (`Q`/`S`), Offline Subject & Background Masking (Lab saliency + joint-bilateral edge refinement) — 1:1 GUI + `rd layer`/`rd heal` + MCP `rune_edit_layers` | ✅ `rev-eng` (#129) | ✅ `rev-ux` (12/12 scenes, #128) | 101/101 passed |
| **Phase 5** | **Film Negative, 4-Folder Sessions, Hot Folder, Proof Margin, Shortcuts & Edit With** | ✅ Complete | Film Negative Conversion (`ProcessingMode` + Stage 0.5 linear inversion + `Pick Film Base`), Capture One 4-Folder Sessions (`Capture/Selects/Output/Trash` + `⌘J` Move to Selects), Hot Folder Auto-Ingest (`HotFolderWatcher` + optional auto-Style), Configurable Proof Margin (`Compact`/`Studio`/`Exhibition`, `⌘⇧M`), Custom Keyboard Shortcuts UI + persistence, `Edit With…` 16-bit TIFF handoff — 1:1 GUI + `rd` CLI + MCP | ✅ `rev-eng` | ✅ `rev-ux` | 109/109 passed |
| **Camera Profiles** | **Dual-Illuminant Calibration, 14 MIT Profiles, Film Simulations & `.dcp`/`.icc`/`.cube` Import** | 🟡 In Progress | Unified `33³` perceptual-shaper 3D LUT (`< 2e-3` Metal GPU↔CPU parity), dual-illuminant `rawler` 738-camera Bradford calibration, 14 bundled MIT profiles (`Rune ProStandard`, 5 Calibration Bases + 9 Film Simulations), pure-Rust `.dcp`/`.icc`/`.icm`/`.cube` import with EXIF camera auto-match & cache invalidation — 1:1 GUI (`Studio Airy`) + `rd profile`/`rd edit --profile` + MCP (`profile_list`/`camera_profile`) | Pending (`prof-rev-eng`) | Pending (`prof-rev-color`) | Updating |

---

## 1. Base Characteristics, RAW Engine & Film Negative Conversion

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| RAW Decoding (Multi-Vendor) | Phase One IIQ, Sony ARW, Canon CR2/CR3, Nikon NEF, Fujifilm RAF, Leica/Apple DNG, Panasonic RW2, OM/Olympus ORF, Pentax PEF, etc. | 🟡 | macOS `CIRAWFilter` (ProRAW/JXL-DNG/OS RAWs) + pure-Rust `rawler` linear Bayer fallback. Needs full X-Trans, dual-pixel, and custom AHD/RCD GPU demosaic. |
| Camera Input ICC Profiles | Camera Generic, Generic V2, **ProStandard** profiles, lighting-specific & manufacturer profiles, custom user profiles (`Other`), `No Color Correction` | 🟡 | Unified `33³` log2-shaper 3D LUT engine (`< 2e-3` Metal GPU↔CPU parity) with 5 bundled MIT calibration bases (`Auto (Camera Matched)`, **`Rune ProStandard`** OKLab hue-invariant, `Camera Generic`, `Apple Native RAW`, `Linear Scientific`), dual-illuminant `rawler` 738-camera Bradford matrix calibration, and pure-Rust **`.dcp` / `.icc` / `.icm` / `.cube`** user profile import with EXIF auto-match across GUI, `rd profile` / `rd edit --profile`, and MCP (`profile_list` / `camera_profile`). Proprietary manufacturer DCP/ICC profiles are user-imported rather than redistributed; `Save as ICC Profile` export still planned. |
| Film Curves (`Base Characteristics > Curve`) | `Auto`, `Film Standard`, `Film Extra Shadow`, `Film High Contrast`, `Linear Response`, `Linear Scientific` | 🟡 | 4 of 6 Base Characteristics curves (`Film Standard`, `Film Extra Shadow`, `Film High Contrast`, `Linear Response`) shipped across GPU WGSL, CPU export, GUI selector, `rd --film-curve`, and MCP `film_curve` (`< 2e-3` parity); `Linear Scientific` is also available via the Camera Profile selector, while automatic curve selection remains planned. |
| Fujifilm Film Simulations | Provia, Velvia, Astia, Classic Chrome, Acros, etc. via Curve dropdown | 🟡 | 9 bundled MIT-licensed analytical Film Simulations (`Portrait Natural`, `Landscape Crisp`, `Editorial Neutral`, `Classic Chrome`, `Vivid Velvia`, `Soft Astia`, `Acros Monochrome`, `Warm Portra 400`, `Tungsten Cine 800T`) with `0–100%` blend strength + `.cube`/`.dcp` user import across GUI, `rd profile` / `rd edit --profile`, and MCP. Proprietary camera-manufacturer in-camera DCP/ICC profiles are user-imported rather than redistributed. |
| Process Engine Versioning | Legacy engine preservation + one-click Upgrade per variant | 🟡 | `DevelopRecipe.schema_version` exists for future migrations; single engine v1 today. |
| **Film Negative Conversion** *(16.7.4)* | `Processing Mode: Photography vs. Film Negative`, `Convert Negative` button, Film Negative Workspace, remapped Exposure/Levels | ✅ | `ProcessingMode::FilmNegative` + Stage 0.5 linear orange-mask neutralization & density/contrast/black-white clip controls (`< 2e-3` GPU↔CPU parity) + `Pick Film Base` eyedropper shipped across GUI `Light` tab, `rd edit` (`--processing-mode`, `--pick-film-base`), and MCP `rune_edit_photo`. |
| Native Grayscale Editing *(16.5)* | Native editing of monochrome/grayscale source files | 🟡 | Converted to `Rgba16Float` cleanly; dedicated grayscale output mode not yet present. |
| E-Commerce ICC Profiles *(Studio)* | Specialized product/color-accurate studio profiles | ➖ | Studio/Enterprise specialization. |

---

## 2. White Balance & Color Calibration

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| Temperature & Tint Sliders | Kelvin (`800–14,000 K`) + Tint (Green–Magenta) | ✅ | Live Kelvin readout (`2,000–14,000 K`, `5,500 K` neutral) + linear Temperature & Tint sliders shipped across GUI, `rd inspect`, and MCP `rune_inspect_photo`. |
| WB Presets Dropdown | `Shot`, `Auto`, `Daylight`, `Cloudy`, `Shade`, `Flash`, `Tungsten`, `Fluorescent` | ✅ | 8 standard illuminant presets (`As Shot`, `Daylight`, `Cloudy`, `Shade`, `Tungsten`, `Fluorescent`, `Flash`, `Custom`) shipped across GUI, `rd edit --wb-preset`, and MCP `wb_preset`. |
| Pick White Balance Eyedropper | Click neutral gray/white point on canvas | ✅ | `3×3` linear-light neutral patch eyedropper shipped across GUI, `rd edit --wb-pick`, and MCP `rune_edit_photo.wb_pick`. |
| Auto White Balance (`A`) | One-click automatic neutral WB | ✅ | Automatic neutral WB (`auto_white_balance`) shipped across GUI (`A`), `rd edit --auto wb`, and MCP `rune_edit_photo`. |
| **Normalize Tool** (`N` / `Shift+N`) | Sample reference color/exposure patch (or enter RGB/HSB) and apply to destination area; Skin Tone presets | ⬜ | Great Capture One signature tool for consistent skin/product matching. |
| **Smart Adjustments** | Reference-based AI Exposure & White Balance matching across batches | ⬜ | Planned after core histogram/exposure statistics. |
| **Match Look** *(16.5)* | AI color/grade transfer from reference image(s) into 4 editable groups (`Normalize`, `Light & Contrast`, `Color Adjust`, `Color Tone`) on Base or Filled Layer | ⬜ | Long-term AI color matching milestone. |

---

## 3. Exposure, High Dynamic Range & Tone Mapping

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| Exposure (`-5.0` to `+5.0 EV`) | Stop-based linear luminance shift | ✅ | Exact `2^EV` linear multiply in `develop.wgsl` + `crates/core` (`-5.0..=+5.0 EV` via `SCALAR_PARAMS`). |
| Contrast (`-100` to `+100`) | Global midtone contrast | ✅ | 18% middle-gray pivoted contrast curve (`-100..=+100`). |
| Brightness (`-100` to `+100`) | Midtone-weighted luminance shift preserving black/white endpoints | ✅ | Midtone-weighted luminance lift (`-1.0..=+1.0`) shipped in `develop.wgsl`, CPU export, GUI, `rd`, and MCP. |
| Saturation (`-100` to `+100`) | Intelligent vibrance-like positive saturation + negative desaturation | ✅ | Separate **Vibrance** (skin/chroma-protecting) and **Saturation** sliders shipped. |
| High Dynamic Range (`Highlight`, `Shadow`, `White`, `Black`) | 4 tonal sliders (`-100..+100`) with single-channel highlight reconstruction | 🟡 | All 4 sliders (`Highlights`, `Shadows`, `Whites`, `Blacks`) shipped; RAW single-channel highlight color reconstruction not yet implemented. |
| Auto Adjust (`Cmd+L`) & Configurable Auto | Per-tool `A` button + `Configure Auto Adjustments` checklist | 🟡 | Per-tool Auto Tone (`A`), Auto WB, and Auto Levels (`0.1%` clip) shipped across GUI, CLI (`--auto`), and MCP; configurable Auto checklist not yet built. |
| Exposure Warning & Shadow Warning Overlays | Configurable red highlight / blue shadow clipping mask directly on Viewer | ✅ | Real-time red highlight (`>= 0.992`) / blue shadow (`<= 0.004`) clipping overlay (`⌘E`) shipped in `develop.wgsl` and GUI toolbar. |

---

## 4. Levels & Tone Curves

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Levels Tool** (`RGB`, `R`, `G`, `B`) | Input Shadow / Midtone / Highlight (0–255), Output Shadow / Highlight (0–255), live histogram | ✅ | Master `RGB` + per-channel `R`, `G`, `B` Levels (`in_black`, `in_mid`, `in_white`, `out_black`, `out_white`) + live per-channel histogram strip shipped across GPU, CPU export, GUI tabs, `rd`, and MCP. |
| Levels Pickers, Auto Levels & Target Levels | Shadow/Highlight eyedroppers, configurable Auto clipping thresholds (`0.10%`), custom Target Levels | 🟡 | Auto Levels (`0.1%` shadow/highlight clip) shipped across GUI, CLI (`--auto levels`), and MCP; Shadow/Highlight eyedroppers & custom Target Levels not yet built. |
| **Tone Curve** — `Luma` Channel | Luminance-only spline curve (contrast without saturation shift) | ✅ | Interactive Fritsch-Carlson monotonic cubic spline baked to `256×1` `Rgba16Float` GPU LUT. |
| **Tone Curve** — `Red`, `Green`, `Blue` Channels | Per-channel RGB tone/color balance curves | ✅ | Interactive 5-tab (`RGB` / `Luma` / `R` / `G` / `B`) draggable knots with ghost curve preview. |
| **Tone Curve** — `RGB` Combined Channel | Traditional RGB curve (boosts saturation alongside contrast) | ✅ | 5th combined `RGB` curve tab baked seamlessly into the `256×1` LUT (`rgb(red(x))`, `rgb(green(x))`, `rgb(blue(x))`) across GPU, CPU, GUI, `rd` (`curves.rgb`), and MCP. |
| Curve Eyedroppers (`Pick RGB`, `Pick Luma`, `Pick Neutralize Point` *16.7.4*) | Click canvas to place curve point or neutralize cast | ⬜ | High-priority interactive viewport picker. |
| Live Cursor Tonal Indicator on Curve | Orange vertical readout line on curve graph tracking hovered pixel | ✅ | Live orange vertical tonal readout line on the Tone Curve graph tracking the hovered canvas pixel's luminance. |

---

## 5. Color Balance, Color Editor & Black & White

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Color Balance** — `3-Way` (`Shadow`, `Midtone`, `Highlight`) | Side-by-side interactive circular hue/sat wheels + independent Lightness (`Y`) sliders | ✅ | Full interactive 3-Way Color Balance wheels + Luma sliders in `Color` tab & `develop.wgsl`. |
| **Color Balance** — `Master` & Single-Wheel Enlarged Tabs | Global hue/sat Master wheel + enlarged individual Shadow/Midtone/Highlight views | 🟡 | Global `Master` wheel + lightness slider (`3-Way` / `Master` switcher in GUI, `master_wheel.*` / `wheels.master` in CLI & MCP) shipped with GPU↔CPU parity; enlarged single-wheel Shadow/Midtone/Highlight tabs not yet built. |
| **Color Editor — Basic (8-Band HSL)** | 8 color swatches with Hue, Saturation, Lightness sliders | ✅ | 8 bands (Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta) with smooth angular blending. |
| **Color Editor — Direct Cursor Tool (`D`)** | Click-and-drag directly on canvas to tweak Hue/Sat/Lightness of sampled color | ⬜ | High-value Capture One workflow enhancer. |
| **Color Editor — Advanced** *(redesigned 16.9)* | Unlimited custom eyedropper color slices, 2D color wheel with draggable wireframe & visual recoloring, `Smoothness`, **`Uniformity`**, **`Target Picker`**, editable **RGB/HSL target values**, `View Selected Color Range` | 🟡 | 4 custom eyedropper slices (`advanced_color.{0..3}`) with target Hue/Sat, `Hue Range`, `Sat Range`, `Smoothness`, `Uniformity`, H/S/L shifts, and live GPU `View Selected Color Range` mask shipped across GUI/GPU/CPU/CLI/MCP; unlimited slices & 2D draggable wireframe wheel still planned. |
| **Color Editor — Skin Tone** | Target skin tone picker + `Uniformity` (`Hue`, `Saturation`, `Lightness`) + `Amount` (`H/S/L`) + `Smoothness` | ✅ | Target Skin Tone eyedropper (`--pick-skin` / `skin_pick`), `Uniformity` (`H/S/L`), `Amount` (`H/S/L`), `Smoothness`, and live GPU `View Selected Color Range` mask shipped across GPU, CPU export, GUI, `rd`, and MCP. |
| Color Editor Actions (`Create Masked Layer`, `Save as ICC Profile`) | Convert color range to Adjustment Layer mask; export color grade as custom ICC profile | ⬜ | Depends on Layers & ICC export. |
| **Black & White Tool** | `Enable B&W`, 6-channel `Color Sensitivity` (`Red`, `Yellow`, `Green`, `Cyan`, `Blue`, `Magenta`), `Split Tones` (Highlights & Shadows Hue/Sat) | ✅ | Dedicated `Enable B&W` toggle, 6-channel color sensitivity mixer (`Red`–`Magenta`), and Highlight/Shadow Split Toning (`split_shadow_hue/sat`, `split_highlight_hue/sat`) shipped across GPU, CPU export, GUI, CLI, and MCP. |

---

## 6. Clarity, Structure, Dehaze & Vignetting

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Clarity** Slider | Broad midtone local contrast (`-100..+100`) | ✅ | Shipped in `Detail` tab & gated spatial pass in `develop.wgsl`. |
| **Clarity Methods** (`Natural`, `Punch`, `Neutral`, `Classic`) | 4 distinct local-contrast & saturation preservation algorithms | ✅ | All 4 Capture One Clarity Methods (`Natural`, `Punch`, `Neutral`, `Classic`) shipped in `develop.wgsl`, CPU export, GUI selector, `rd`, and MCP (`clarity_method`). |
| **Structure** Slider | Fine micro-contrast texture enhancement / softening | ✅ | Fine micro-contrast companion to Clarity shipped in `develop.wgsl`, CPU export, GUI, CLI, and MCP. |
| **Dehaze** (`Amount` + `Auto` / `Pick Haze Shadow Tone`) | Atmospheric haze removal/addition with airlight shadow tone control | 🟡 | Atmospheric `Dehaze` slider (`-1.0..=+1.0`) shipped in GPU/CPU/GUI/CLI/MCP; `Auto` & `Pick Haze Shadow Tone` not yet built. |
| **Vignetting** (`Amount` `-4..+4 EV` + `Elliptic on Crop`, `Circular on Crop`, `Circular`) | Optical corner darkening/brightening with crop-aware geometry modes | ✅ | Optical vignetting with all 3 Capture One crop-aware geometry modes (`Elliptic on Crop`, `Circular on Crop`, `Circular` via `vignette_method`) shipped across GPU, CPU export, GUI, CLI, and MCP. |

---

## 7. Detail, Noise Reduction, Film Grain & Optical Artifacts

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Sharpening** (`Amount`, `Radius`, `Threshold`, `Halo Suppression`) | Full 4-parameter unsharp/deconvolution sharpening | ✅ | Full 4-parameter sharpening (`sharpen`, `sharpen_radius` `0.5–4.0`, `sharpen_threshold`, `sharpen_halo`) shipped across `develop.wgsl`, CPU export, GUI, CLI, and MCP. |
| **Noise Reduction** (`Luminance`, `Details`, `Color`, `Single Pixel`) | Manual RAW noise reduction controls | 🟡 | Separate Luminance (`noise_reduction`) and Color (`nr_color`) bilateral noise reduction shipped across GPU/CPU/GUI/CLI/MCP; `Details` & `Single Pixel` sliders still planned. |
| **AI Enhanced Denoise** *(16.8 / +30% faster 16.9)* | Neural high-ISO Bayer RAW denoise with `Impact` slider | ⬜ | Ideal Apple Neural Engine (`coreml-native`) feature so GPU stays free at 120Hz. |
| **Film Grain** (6 Types + `Impact` & `Granularity`) | `Fine`, `Silver Rich`, `Soft`, `Cubic`, `Tabular`, `Harsh` grain profiles | ✅ | All 6 Capture One Film Grain profiles (`Fine`, `Silver Rich`, `Soft`, `Cubic`, `Tabular`, `Harsh`) + `Impact` (`grain`) & `Granularity` (`grain_size`) shipped across GPU, CPU export, GUI, CLI, and MCP. |
| **Moiré** (`Amount`, `Pattern`) | False-color interference suppression on fabrics/architecture | ⬜ | Local chroma frequency filter. |
| **Purple Fringing** (`Amount`) | High-contrast edge fringe neutralization | ✅ | High-contrast purple/magenta edge fringe desaturation (`lens.purple_fringing`) shipped across `develop.wgsl`, CPU export, GUI Lens Correction card, `rd`, and MCP. |
| **Dust Removal / AI Remove Dust** *(16.2+)* | One-click AI sensor dust removal (`Opt+O`) + manual Dust & Spot cursor tools (`O`) | ⬜ | Planned alongside Heal/Clone layers. |

---

## 8. Lens Correction & Lens Cast Calibration (LCC)

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Lens Profiles** (Custom, Manufacturer Embedded, Generic) | Automatic camera/lens EXIF profile matching (incl. Canon RF, Sony, Fuji, M4/3) | ⬜ | Integrate open-source **Lensfun** database + EXIF lens metadata + embedded DNG OpcodeLists. |
| **Chromatic Aberration** (+ `Analyze` button) | Automatic & analyzed lateral CA removal during demosaic/develop | 🟡 | Manual radial R/B channel lateral CA correction (`lens.chromatic_aberration`) shipped across GPU, CPU export, GUI, `rd`, and MCP; automatic `Analyze` button still planned. |
| **Distortion**, **Sharpness Falloff**, **Light Falloff** | Barrel/pincushion, corner softness, and optical vignetting sliders + `Hide Distorted Areas` | 🟡 | Radial barrel/pincushion `lens.distortion` UV warp & optical `cos^4` `lens.light_falloff` corner lift shipped across GPU/CPU/GUI/CLI/MCP; `Sharpness Falloff` & `Hide Distorted Areas` still planned. |
| **Diffraction Correction** | Aperture-aware optical deconvolution for small f-stops | ⬜ | GPU deconvolution pass. |
| **Lens Movement Tab** | Manual Focal Length, Aperture, Shift X/Y (mm) for technical/tilt-shift lenses | ⬜ | Niche technical-camera feature. |
| **LCC (Lens Cast Calibration)** | Flat-field reference plate profiling for Color Cast, Uniform Light, Sensor Dust, Wide-Angle data | ⬜ | Medium-format/technical camera workflow; clean flat-field division pass if added later. |

---

## 9. Composition & Geometry

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Crop Tool** | Interactive crop handles, presets (`Unconstrained`, `Original`, `Output`, `1:1`, `4:3`, `3:2`, `16:9`, `5:4`, `4:5`, custom), `Switch Orientation` (`Shift+X`), `Crop Outside Image`, auto-orientation sync *(16.8.5)* | 🟡 | Persistent non-destructive Crop with draggable viewport handles & 8 aspect presets (`Free`, `Orig`, `1:1`, `4:3`, `3:2`, `16:9`, `5:4`, `4:5`) shipped across GUI/GPU/export/CLI/MCP; `Switch Orientation`, custom ratios, and `Crop Outside Image` still planned. |
| **AI Crop** *(16.4+)* | Reference-based face/subject crop matching across batches; Studio margin/alignment rules | ⬜ | Can use Apple Vision face/saliency detection on macOS. |
| **Grid & Guides Tools** (`Cmd+G`) | `Rectangular` (Rule of Thirds / N×M), `Golden Ratio`, `Fibonacci Spiral` (mirror/flip), `Follow Crop`, draggable Guides | 🟡 | Composition Grid overlays (`None`, `Rule of Thirds`, `Golden Ratio`, `Diagonal`) shipped inside Crop frame; Fibonacci Spiral & draggable Guides still planned. |
| **Rotation & Flip** | 90° CW/CCW, fine angle slider, `Straighten` horizon cursor tool, Horizontal/Vertical Flip, **`Auto Rotate`** | 🟡 | Persistent `Straighten` (`±45°`), 90° CW/CCW rotate, Flip H/V, and trackpad `on_rotate` shipped in GPU/export/CLI/MCP; horizon draw-line tool & `Auto Rotate` still planned. |
| **Keystone Correction** (`K`) | `Vertical`, `Horizontal`, `Full` 4-point interactive guides, `Auto`, `Amount` (80% default), `Aspect` (`-50..+100` incl. anamorphic), `Skew`, Keystone Loupe *(16.9.1)* | 🟡 | 3×3 perspective homography (`geometry.keystone_vertical`, `keystone_horizontal`, `keystone_aspect`, `keystone_amount` default `80%`) shipped across GPU, CPU export, GUI, `rd`, and MCP; interactive 4-point canvas guides, `Auto`, `Skew`, and Keystone Loupe still planned. |

---

## 10. Multi-Image Computational

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **HDR Merge** | Merges bracketed RAW exposures into 16-bit linear DNG (`Auto Align`, `Auto Adjust`, deghosting) | ⬜ | Fits our linear `Rgba16Float` pipeline well for future Phase 4+. |
| **Panorama Stitch** | Multi-row RAW stitching to 16-bit linear DNG (`Spherical`, `Cylindrical`, `Perspective`, `Panini`; `25%–100%` scale) | ⬜ | Future computational milestone. |
| **Focus Stacking Integration** | Sequence export bridge to Helicon Focus | ➖ | Covered via future `Edit With` / external app handoff. |

---

## 11. Layers, Masks & Local Adjustments

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Adjustment Layers** (up to 16) | `Image Layer` (base) + `Empty` / `Filled` Adjustment Layers, Opacity (`0–100%`), visibility, reorder, rename, Duplicate Layer | 🟡 | Up to 8 non-destructive `AdjustmentLayer`s (`Empty`/`Filled`/masked) with Opacity (`0–100%`), visibility, reorder ↑/↓, duplicate, delete, and 11 local adjustments shipped across GPU, CPU export, GUI `Layers` tab, `rd layer`, and MCP `rune_edit_layers` (`< 2e-3` parity); 16-layer scaling still planned. |
| **Manual Brush (`B`) & Eraser (`E`)** | Size, Hardness, Opacity, Flow, Airbrush, **Auto Mask** (edge-aware), Pen Pressure, Link Brush/Eraser | 🟡 | Circular soft/hard `Brush` (`B`) & `Eraser` (`E`) dabs (Size, Hardness, Opacity) baked into `1024×512` `R8Unorm` GPU mask atlas shipped; Flow, Airbrush, edge-aware `Auto Mask`, and stylus pen pressure still planned. |
| **Magic Brush & Magic Eraser** | Color/tone/texture-aware smart painting (`Tolerance`, `Refine Edge`, `Sample Entire Photo`) | ⬜ | GPU flood/edge-aware region growing. |
| **Linear (`L`) & Radial (`T`) Gradient Masks** | Parametric 3-line / 3-ring controls, rotate/asymmetric feather, Invert, Rasterize | ✅ | Linear (`L`) & rotated elliptical Radial (`T`) gradient masks with interactive vector guides, feather control, and invert shipped across GPU, CPU export, GUI, `rd layer`, and MCP. |
| **Luma Range Mask** | Luminosity ramp with fall-off handles, `Radius`, `Sensitivity`, `Invert Range` | ✅ | Live scene-luminance range mask (`low`, `high`, `low_falloff`, `high_falloff`, invert) evaluated identically per-pixel on GPU & CPU shipped across GUI, `rd layer`, and MCP. |
| **Combine Masks** *(16.7+)* | Non-destructive **Add**, **Subtract**, **Intersect** boolean mask stacks inside a single layer | ✅ | Non-destructive `Add`, `Subtract`, and `Intersect` composable mask stacks (up to 4 masks per layer) shipped across GPU, CPU export, GUI `COMBINE MASKS` card, `rd layer mask-add`, and MCP `rune_edit_layers`. |
| **Mask Operations & Display Modes** | Copy Mask From, Invert, Clear, Fill, Feather, Refine, Rasterize; Display Always / While Drawing / Never / Grayscale (`Opt+M`); Export Masks to PSD | 🟡 | Invert, Clear, Fill + GPU Rubylith Red (`M`) & Grayscale (`⌥M`) mask display overlays shipped; Copy Mask From, Refine Mask, and PSD mask export still planned. |
| **Heal (`Q`) & Clone (`S`) Layers** | Dedicated repair layers, multi-stroke Auto Source Point healing, manual draggable source markers | 🟡 | Non-destructive Stage-0 Spot Heal (`Q`) & Clone (`S`) stamps (up to 16) with auto-selected source patches, target→source canvas guides, radius/feather/opacity shipped across GPU, CPU export, GUI, `rd heal`, and MCP; multi-stroke brush heal layers still planned. |
| **Style Brushes** | One-stroke masked layer creation with bundled local adjustments | ⬜ | Pairs with Layers + Styles. |

---

## 12. AI Masking & AI Portrait Retouching Suite

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **AI Subject & Background Masking** *(16.3+)* | One-click primary Subject / Background masks; dynamically recomputes in Styles/Batch | 🟡 | One-click offline **Subject & Background** `256×256` masks (`generate_subject_mask`, GrabCut-inspired Lab saliency + joint-bilateral edge refinement, exact subject/background complement) shipped across GUI, `rd layer --kind ai-subject|ai-background`, and MCP `rune_edit_layers`; native Apple Neural Engine (`objc2-vision`) model upgrade planned next. |
| **AI Select & AI Eraser** | Hover-preview click/box arbitrary object & sky segmentation | ⬜ | SAM 2 / CoreML / MLX on Apple Silicon. |
| **People Masking** *(16.5 / 16.7)* | 9 anatomical sub-masks (`Face Skin`, `Body Skin`, `Hair`, `Eyebrows`, `Pupil+Iris`, `Sclera`, `Lips`, `Teeth`, `Clothes`) with `Separate Masks` | ⬜ | CoreML / Vision human-parsing models on ANE. |
| **Retouch Face Skin** *(16.6 / 16.7)* | Up to 32 faces (`Retouch Selector`), `Impact`, `Blemish Removal`, `Blemish Protection` brush, `Even Skin` + `Texture`, `Include Neck Area`, `Dark Circles`, `Contouring` | ⬜ | Future AI Portrait Retouch module. |
| **Retouch Eyes & Teeth** *(16.7)* | Left/Right `Iris` & `Sclera`; Teeth `Brightness` & `Desaturation` | ⬜ | Future AI Portrait Retouch module. |
| **Stray Hair, Body Skin & Skin Tone Retouching** *(16.9)* | `Outer Hair`/`Inner Hair` + Protection Brush; Body Skin + `Texture`; Skin Tone `Target Picker` + `Uniformity`/Temp/Tint/Sat/Lightness | ⬜ | Future AI Portrait Retouch module. |
| **Replace Background / Actions** *(Photoroom, Pixelz, Gemini, Krea)* | Cloud API connectors for commercial e-commerce post-production | ➖ | Commercial Studio cloud add-on; could be optional plugin later. |

---

## 13. Organization: Sessions, Catalogs, Variants & Stacks

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Folder / Session Workflow** | Open any folder directly with sidecar edits (`.cos` / `.rune-edit.json`) | ✅ | `SessionCatalog` + folder scanning (`⌘⇧O`) + `.rune-edit.json` sidecar persistence (`⌘S`). |
| **4-Folder Session Structure & Templates** | Capture / Selects (`⌘J`) / Output / Trash folders, Session Favorites, Session Albums/Smart Albums, Session Templates, Session Builder *(16.6+)* | 🟡 | Capture One 4-folder Session initialization (`Capture/`, `Selects/`, `Output/`, `Trash/`) + non-clobbering `Move to Selects` (`⌘J`) with sibling `.rune-edit.json` sidecar shipped across GUI `NSMenu`, `rd session init`, and MCP `rune_manage_workflow` (`session_init`); Session Albums/Smart Albums & Templates still planned. |
| **Centralized Catalogs** | Managed vs. Referenced SQLite Catalogs, Projects, Groups, Albums, Smart Albums, Offline Previews, Locate/Sync/Verify/Backup, Lightroom/Aperture Import | ⬜ | Planned as optional catalog index over SQLite after Sessions mature. |
| **Virtual Variants (`New Variant`, `Clone Variant`)** | Multiple non-destructive edit versions pointing to 1 source RAW | ✅ | Multiple non-destructive `PhotoVariant`s (`New`, `Clone`, `Select`, `Delete`, v3 backward-compatible `.rune-edit.json`) shipped across GUI Variant Strip, `rd variant`, and MCP `rune_manage_workflow`. |
| **Variant Stacks, Primary vs. Selected & Multi-Edit** | Thick/thin selection borders, `Edit All Selected Variants` toggle, Promote/Demote stack order | ⬜ | Enables true Capture One batch editing. |
| **Adjustments Clipboard (Selective Per-Tool Copy/Apply)** | Granular checkbox list per tool + Autoselect Adjusted + Additive Layer sync | 🟡 | Capture One-style 14-group Selective Adjustments Clipboard (incl. Film Negative & Layers/Retouch) + `Autoselect Adjusted` shipped across GUI, `rd copy-look`, and MCP `rune_manage_workflow`; additive per-layer merge still planned. |
| **Enhanced Image Package (`.eip`)** | Self-contained RAW + edits + ICC archive | ⬜ | Optional archive export later. |

---

## 14. Culling, Ratings, Filtering & Search

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Filmstrip / Browser Views** | Bottom Filmstrip, Grid View, List View, Secondary Display | 🟡 | Collapsible bottom Filmstrip (`F`) + multi-column **Grid Browser (`G`)** with thumbnails, stars, tags, variant badges & EXIF shipped; List View & Secondary Display still planned. |
| **Star Ratings (`0–5`)** | Keyboard shortcuts `0`–`5` + clickable thumbnail stars | ✅ | Shipped (`0`–`5` shortcuts + interactive star bar on filmstrip & grid cards). |
| **Pick / Reject & 7 Color Tags** | Pick/Selects + Red/Orange/Yellow/Green/Blue/Purple/Pink tags + Auto-Advance | ✅ | Pick (`P`) / Reject (`X`), all 7 Capture One Color Tags (`⌥1`–`⌥7`, `⌥0` clear, `rd cull --color-tag`, MCP), and **Auto-Advance** toggle shipped! |
| **Filters Tool & Search** | Quick filter bar + faceted counts (Camera, Lens, ISO, Date, Orientation, Tag, Rating) + Boolean Advanced Search | 🟡 | `All | Picks | ★3+`, 7 Color Tag filter pills, and case-insensitive text search (filename/camera/lens/preset/tag) shipped in GUI & `rd session scan`; faceted counts & Boolean query builder still planned. |
| **Dedicated Cull View & Group Similar** | Instant preview culling, AI burst similarity grouping with slider, Automatic Face Zoom | ⬜ | Huge workflow win for wedding/event/portrait photographers. |
| **Assisted Review (AI Culling)** *(16.8 / 16.9)* | Auto-flags closed eyes, out-of-focus eyes, blank frames; per-face Viewer indicator | ⬜ | Can leverage Apple Vision face/eye-landmark + sharpness scoring on macOS. |
| **Compare Variant & Multi-View** | Pin orange-bordered reference variant side-by-side against cycling candidates; multi-image viewer | 🟡 | Capture One-style **Compare Variant (`⌘⏎`)** with pinned `#E87A2A` orange-bordered Reference frame side-by-side against cycling candidates/variants shipped; >2 multi-image viewer still planned. |
| **Focus Mask Overlay** | Real-time acutance highlight overlay in Viewer & Browser | ✅ | Real-time green Sobel/Laplacian acutance Focus Mask overlay (`⌘M`) shipped in `develop.wgsl` and GUI toolbar. |

---

## 15. Metadata, Keywords, Naming Tokens & Ingest

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **EXIF / IPTC / GPS Metadata Panel** | Read camera/lens EXIF, edit IPTC (Caption, Creator, Copyright, Getty fields), batch copy metadata | 🟡 | macOS `ImageIO` EXIF telemetry (camera, lens, focal length, aperture, shutter, ISO) shipped in status bar, `rd inspect`/`rd analyze`, and MCP; IPTC/GPS editor still planned. |
| **XMP Sidecar Sync** | 2-way Full Sync / Load / Manual `.xmp` sidecar metadata & crop/rotation interop | ⬜ | Important for interoperability with Lightroom/Photo Mechanic. |
| **Keywords & Hierarchical Keyword Libraries** | Parent/child `|` trees, `.txt` import/export, export filter toggles | ⬜ | Pairs with Catalog/Session search. |
| **Metadata Presets** | Reusable copyright/creator templates applied on import/tether | ⬜ | Pairs with Metadata panel. |
| **Dynamic Naming Tokens & Batch Rename** | Pill tokens (Counters, Date, EXIF, IPTC, Job/Collection/Original Name) across Import, Rename, Export, Watermarks | 🟡 | Traversal-safe Dynamic Naming Tokens (`{ImageName}`, `{Counter}`, `{Date}`, `{Camera}`, `{Lens}`, `{ISO}`, `{Variant}`, `{Recipe}`) shipped in GUI Export, `rd export --naming-template`, and MCP; in-place Batch Rename & IPTC tokens still planned. |
| **Import Dialog** | Card ingest, token subfolders, dual-drive `Backup To`, duplicate exclusion, Style on import, eject | ⬜ | Folder-first workflow skips mandatory import, but an optional SD-card ingest dialog is very helpful. |
| **C2PA Content Credentials** *(16.9.1)* | Cryptographic provenance signing on JPEG/TIFF export | ⬜ | Open-source `c2pa-rs` crate makes this surprisingly clean later! |
| **Sequences** | Sequence IDs, dynamic sequence subfolders on import/tether/export | ➖ | Specialized studio/focus-stack workflow. |

---

## 16. Styles, Presets, Speed Edit, Customization & Automation

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Built-in & Custom Styles** | Built-in looks + Save User Style (selective tools) + `.costyle` / `.costylepack` import | 🟡 | 6 curated built-in Styles + traversal-safe **User Styles** (`Save`/`List`/`Apply`/`Delete` with selective groups & `Autoselect Adjusted`) shipped across GUI, `rd style`, and MCP; `.costyle` / `.costylepack` import still planned. |
| **Stacked Styles & Per-Tool Presets** | Layer multiple Styles cumulatively; per-tool preset dropdowns (e.g., Curve presets, WB presets) | 🟡 | **Additive Style Stacking** (`--stack`, `stack: true`, GUI Stack toggle + Strength slider) and WB Presets shipped; per-tool curve/levels preset dropdowns still planned. |
| **Speed Edit** (Key + Drag / Scroll) | Hold hotkey (`Q`=Exposure, `W`=Contrast, `A`=Highlights, `S`=Shadows, etc.) + drag/scroll anywhere on screen | ✅ | Shipped (`Q` Exposure, `W` Contrast, `E` Brightness, `R` Saturation, `A` Highlights, `S` Shadows, `D` Whites, `Z` Clarity, `T` Temp, `Y` Tint + floating HUD). |
| **Custom Workspaces, Tool Tabs & Floating Tools** | Reorder/detach panels, custom tool tabs, save/switch layouts | 🟡 | Collapsible Filmstrip (`F`) & Inspector (`I`) + 6 clean OpenCode tabs shipped (`Light` \| `Color` \| `Detail` \| `Lens` \| `Layers` \| `Style`); full panel customization later. |
| **Custom Keyboard Shortcuts** | User-editable shortcut sets + HTML cheat-sheet export | 🟡 | Rich macOS default shortcuts + native `NSMenu` key equivalents + interactive `Settings` (`⌘,`) **Keyboard Shortcuts** remapping panel with persistent `~/.config/rune-darkroom/settings.json` & `Reset Defaults` shipped; HTML cheat-sheet export still planned. |
| **Tangent Grading Panels** | Hardware colorist control surface support | ➖ | Niche hardware integration. |
| **AppleScript / Background Scripts / Plugin SDK** | Event hooks (`CO_CaptureDone`, `CO_BatchJobFinished`), AppleScript dictionary, C++/plugin SDK | 🟡 | Agent-native `rd` CLI (declarative JSON `batch run`, auto-JSON/NDJSON, CIEDE2000 `diff --assert`) + localhost MCP (`2025-11-25`) server shipped; event hooks/plugin SDK later. |

---

## 17. Viewer, Scopes, Proofing & Display Color Management

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Live RGB + Luma Histogram** | Real-time tonal distribution + shadow/highlight clipping telemetry | ✅ | Anti-aliased RGB + Luma area histogram with `BLK %` / `WHT %` clip readouts pinned in inspector. |
| **RAW Exposure Evaluation Meter** | Pre-adjustment sensor histogram + $\pm 2\text{ EV}$ meter | ⬜ | Useful once RAW sensor stats are exposed. |
| **Before / After (Split & Full View)** | Draggable split divider + instant momentary full-canvas Original toggle (`\`) | ✅ | Crisp GPU split divider with frosted HUD badges + `\` toggle shipped. |
| **Zoom, Pan & Smooth Canvas** | Fit, 100%, 200%, pinch-to-zoom, pan, rotate | ✅ | Locked 120Hz Metal GPU viewport (`Fit | 150% | 200%`), native trackpad pinch (`on_magnify`), rotate (`on_rotate`), two-finger double-tap (`on_smart_magnify`), wheel/drag pan. |
| **Color Readouts (RGB / Lab / CMYK / HSL)** *(HSL added 16.7)* | Live cursor pixel probe + multiple pinned Viewer readout markers | 🟡 | Live cursor RGB / HSL / CIE Lab readout + up to **4 numbered Pinned Color Readout Markers (`#1..#4`)** updating live on slider drags shipped; CMYK readout still planned. |
| **Loupe Tool** | High-magnification floating pixel inspector in Viewer & Browser | ✅ | Single-pass **120Hz GPU Loupe (`O` / `⌥L`)** circular magnifier with neutral rim & center crosshair shipped in `develop.wgsl` and GUI viewer. |
| **Annotations Tool** | Freehand sketch notes over Viewer, exportable as PSD layer | ⬜ | Studio retoucher communication feature. |
| **Proof Margin & Slideshow** | Configurable mat margin & full-screen slideshow mode | 🟡 | Configurable Capture One-style **Proof Margin** (`Compact` 16pt / `Studio` 48pt / `Exhibition` 84pt, `⌘⇧M`) shipped in GUI Viewer; full-screen slideshow mode still planned. |
| **ICC Soft-Proofing & Recipe Proofing** | Real-time Viewer simulation of target ICC profile, output scale, sharpening, watermark, and JPEG compression | ⬜ | Requires `moxcms` / ColorSync output profile transform. |
| **Wide-Gamut Display P3 / ColorSync** | Native P3 monitor color management | ⬜ | **Future Milestone** — internal pipeline is already scene-referred linear `Rgba16Float`; awaits P3/EDR swapchain in Rune (or patched `MetalSurface`). |
| **Native XDR / EDR HDR Display & Gain Maps** | >1.0 peak brightness rendering on Apple XDR displays + ISO 21496-1 Gain Maps | ⬜ | **Differentiator Opportunity** — Capture One *still* does not support EDR display or Gain Map HDR as of 16.9.1! |

---

## 18. Export, Output Recipes, External Editing, Contact Sheets & Print

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Single Image Export (JPEG, PNG 8/16, TIFF 16, WebP)** | Full-resolution graded export matching Viewer color science | ✅ | `rayon`-parallel export (`⌘⇧E`, `rd export`, MCP) to JPEG, 8-bit/16-bit PNG, 16-bit TIFF, and WebP with full spatial-detail & geometry GPU parity! |
| **Simultaneous Multi-Recipe Export Queue** | Batch-check multiple Export Recipes (e.g., Full TIFF + Web JPEG + Proof) in one pass with background queue | ✅ | Simultaneous Multi-Recipe Export (`Full-Res JPEG`, `Web 2048px`, `Archival 16-bit TIFF`, plus custom JSON specs) with pre-flight collision/traversal guards shipped across GUI Export drawer, `rd export --recipes`, `rd batch run`, and MCP `rune_cull_or_export`. |
| **Additional Export Formats** | PSD (8/16-bit), JPEG QuickProof, JPEG 2000, DNG, Originals + EIP *(Note: C1 lacks WebP/AVIF/JXL export)* | 🟡 | Native **WebP** export already shipped (surpassing C1 16.9.1); **AVIF and 16-bit JPEG XL** still planned. |
| **Resize / Scaling Modes & Output Sharpening** | Fixed %, Width, Height, Long/Short Edge, Dimensions + Screen & Print viewing-distance Output Sharpening | 🟡 | Linear-light Catmull-Rom Long-Edge resize (`max_edge`) & screen `output_sharpen` shipped across GUI/CLI/MCP; Fixed %/W/H & Print sharpening still planned. |
| **Output ICC Profile Conversion & Metadata Strip** | Convert to sRGB / Display P3 / Adobe RGB / CMYK; toggle EXIF/GPS/Copyright/Keywords | 🟡 | Exact CIE 1931 `sRGB` & `Display P3` matrix conversion (`--color-space display-p3`) + `strip_metadata` toggle shipped across GUI/CLI/MCP; full embedded ICC profiles (`moxcms`) & Adobe RGB/CMYK still planned. |
| **Text & Image Watermarking** | Token-based Text or PNG logo watermark with scale, opacity, X/Y offset | 🟡 | Anti-aliased **Text Watermarks** (text, opacity, scale, 5 anchor positions) shipped across GUI Export, `rd export --watermark`, and MCP; PNG image logo watermarking still planned. |
| **External Editing (`Edit With` / `Open With`)** | Round-trip TIFF/PSD/Affinity `.af` to Photoshop or Affinity Photo with auto re-import | 🟡 | Non-destructive `Edit With External Editor…` exports a full-resolution 16-bit TIFF sibling safely (never modifying the source RAW) and launches it via macOS `open`; auto-reload watcher & PSD/Affinity round-trip still planned. |
| **Contact Sheets** *(overhauled 16.7+)* | Branded multi-page PDF/JPEG contact sheets with cover, header/footer, ratings, and color tags | ⬜ | Nice Tier 3/4 export module. |
| **Print Engine** | Print templates, grid layout, margins, printer ICC profiles, rendering intent, BPC, print sharpening | ⬜ | Lower priority for digital-first MVP. |

---

## 19. Tethering, Live View, Studio Production, Collaboration & Mobile

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Hot Folder Watch (Auto-Ingest)** | Automatically display & grade new images dropped into a watched folder | ✅ | Symlink/dotfile-safe `HotFolderWatcher` (two-poll byte-size stability + optional auto-apply Style on arrival) shipped in GUI Session Browser / File menu and `rd session watch` (`--style`, `--once`). |
| **Wired & Wireless Camera Tethering** | Direct USB/Wi-Fi/Ethernet SDK control for Canon, Nikon, Sony, Fujifilm, Leica, Panasonic, Phase One, Hasselblad + **ReTether** + **2nd Gen Canon Wireless** | ➖ | Requires proprietary camera SDKs (`EDSDK`, `CrSDK`, `Nikon MAID`) or `libgphoto2`—major separate plugin track later. |
| **Live View, Camera Focus & Focus Meter** | Real-time sensor stream, remote AF/focus nudging, 3-box contrast Focus Meter, Composition Mode | ➖ | Tied to camera tether SDKs. |
| **Overlay Tool** | Semi-transparent PNG/PSD/TIFF art-direction mockup over Viewer & Live View | ⬜ | Easy standalone Viewer overlay even without tethering! |
| **Capture One Live / Live for Studio / Multi-User Sessions** *(16.9 GA)* | Cloud web client review, offline LAN iPad review, real-time LAN multi-workstation Session editing | ➖ | Cloud/LAN studio infrastructure—out of scope for v1. |
| **Enterprise & Cultural Heritage (CH)** | Jamf Pro deployment, Admin Portal, Tool Locks + PIN, Barcode/QR scanner, Copy Stand, Slipstream, PPI Assist, AutoColumn | ➖ | Specialized Enterprise/Museum editions. |
| **Capture One Mobile (iPad / iPhone) & Cloud Transfer** | Mobile RAW editing/tethering + Cloud album transfer | ➖ | Future cross-platform desktop (Windows/Linux) comes first; Rune iOS exists long-term. |

---

## Recommended Next Implementation Priorities (PR Roadmap)

If you want to close the highest-impact gaps for real-world photographers fastest (and great places for community PRs!):

### Tier 1 — Daily-Driver Photographer Essentials (Completed!)
1. ✅ **Interactive Crop, Straighten & Composition Grid (`Rule of Thirds` / `Golden Ratio` / `Diagonal`)** + persistent Rotation/Flip in `DevelopRecipe`, `develop.wgsl`, and export.
2. ✅ **EXIF / Camera Metadata Readout** (Camera model, Lens, Focal Length, Aperture, Shutter, ISO via macOS `ImageIO`) in the Status Bar.
3. ✅ **White Balance Eyedropper (`3×3` patch) + Auto WB + Auto Exposure/Tone + Auto Levels (0.1% clip)**.
4. ✅ **Speed Edit (Key + Drag / Scroll)** (`Q` Exposure, `W` Contrast, `E` Brightness, `R` Saturation, `A` Highlights, `S` Shadows, `D` Whites, `Z` Clarity, `T` Temp, `Y` Tint + floating HUD).
5. ✅ **Viewer Overlays & Probes:** Exposure/Shadow Clipping Warning (`⌘E`), **Focus Mask (`⌘M`)**, and **Live RGB / HSL / Lab Color Readout** under cursor.
6. ✅ **Levels, Brightness, Structure, Dehaze, 6-Channel Black & White Mixer**, plus **WebP / Resize / Output Sharpening** export.

### Tier 2 — Signature Capture One Color & Workflow Depth
7. ✅ **Phase 1 Depth (Shipped):** Base Characteristics Film Curves, Kelvin WB + Presets, Per-Channel R/G/B Levels, Combined RGB Tone Curve + Live Tonal Indicator, Master Color Wheel, B&W Split Tones, Clarity Methods, Full 4-param Sharpening, Color NR, and 6 Film Grain profiles + Granularity.
8. ✅ **Phase 2 Depth (Shipped):** 3-Tab Color Editor (4-slice Advanced Color Editor + Uniformity + `View Selected Color Range` + Skin Tone Uniformity), Crop-Aware Vignetting, Optical Lens Correction (`Distortion`, `CA`, `Purple Fringing`, `Light Falloff`), and 4-param Keystone Perspective.
9. ✅ **Phase 3 Workflow Depth (Shipped):** Virtual Variants (`New`/`Clone`/`Select`/`Delete`), Compare Variant (`⌘⏎` orange border), 7 Color Tags + Auto-Advance, Grid Browser (`G`), Selective Adjustments Clipboard (`Autoselect Adjusted`), User & Stacked Styles, 120Hz GPU Loupe (`O` / `⌥L`) + 4 Pinned Color Readouts, Dynamic Naming Tokens, Text Watermarks, Display P3 matrix export, and Simultaneous Multi-Recipe Export Queue.
10. ✅ **Camera Profiles & Film Simulations Engine (Shipped):** Unified `33³` perceptual-shaper 3D LUT engine (`< 2e-3` Metal GPU↔CPU parity), dual-illuminant `rawler` 738-camera Bradford matrix calibration, 14 bundled MIT profiles (`Rune ProStandard`, 5 Calibration Bases + 9 Film Simulations), and pure-Rust external `.dcp` / `.icc` / `.icm` / `.cube` import with EXIF auto-match across GUI, `rd profile`/`rd edit --profile`, and MCP (`profile_list`/`camera_profile`). Proprietary manufacturer DCP/ICC files are user-imported rather than redistributed.
11. **Export Polish:** Embedded ICC profiles (`moxcms`), PNG Logo Watermarks, plus **AVIF / JPEG XL**.
12. **Lensfun Camera/Lens Profiles** + interactive 4-point Keystone guides & Auto CA Analyze.

### Tier 3 — Local Adjustments, Masks & AI Masking
12. ✅ **Phase 4 Layers, Masks & Heal/Clone (Shipped):** Adjustment Layers (up to 8), Combine Masks (`Add`/`Subtract`/`Intersect`), Linear (`L`) / Radial (`T`) / Luma Range / Brush (`B`) & Eraser (`E`), Rubylith (`M`) & Grayscale (`⌥M`) mask views, Spot Heal (`Q`) & Clone (`S`), and offline Subject & Background masking across GUI, GPU/CPU export, `rd layer`/`rd heal`, and MCP `rune_edit_layers`.
13. **Apple Neural Engine AI (`objc2-vision` / `coreml-native`):** Upgrade Subject/Background to ANE `VNGenerateForegroundInstanceMaskRequest`, add People anatomical sub-masks, AI Dust Removal, and AI Enhanced Denoise.

### Tier 4 — Final Workflow Parity, Leapfrog Features & Cross-Platform
14. ✅ **Phase 5 Studio & Film Workflow (Shipped):** Film Negative Conversion (`ProcessingMode` + Stage 0.5 linear inversion + `Pick Film Base`), Capture One 4-Folder Sessions (`Capture/Selects/Output/Trash` + `⌘J` Move to Selects), Hot Folder Auto-Ingest (`HotFolderWatcher` + optional auto-Style), Configurable Proof Margin (`Compact`/`Studio`/`Exhibition`, `⌘⇧M`), Custom Keyboard Shortcuts UI + persistence, and non-destructive `Edit With…` 16-bit TIFF handoff.
15. **Wide-Gamut Display P3 + Native XDR/EDR HDR Display** + **ISO 21496-1 Adaptive Gain Map** export (beating Capture One 16.9!).
16. **2-Way XMP Sidecar Sync**, **Windows (DX12) & Linux (Vulkan) CI**, and GPU WGSL AHD/RCD demosaic.

---

## Sources
- Capture One Official Help Center (`https://support.captureone.com/hc/en-us` — live index of 10 categories, 84 sections, 1,299 published articles as of Nov 25, 2026)
- Capture One Desktop Release Notes (16.2 through **16.9.1**, `https://support.captureone.com/hc/en-us/sections/360000331817-Capture-One-desktop`)
- Capture One Product Features (`https://www.captureone.com/en/features`)
