# Capture One Pro (16.9.1) vs. Rune Darkroom — Exhaustive Feature Parity Matrix

> **Last updated:** November 25, 2026  
> **Reference baseline:** Capture One Pro / Studio / Enterprise / Mobile **16.9.1** (verified against all 10 categories, 84 sections, and 1,299 published articles on `support.captureone.com` plus official 16.2–16.9.1 release notes).  
> **Rune Darkroom baseline:** `main` (`0.1.0`, Rust + Anomaly Rune + single-pass `Rgba16Float` WGSL / Metal backend).

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
| 1 | Base Characteristics & RAW Engine | 🟡 | macOS `CIRAWFilter` + `rawler` linear FP16 | Camera ICC / ProStandard profiles, Film Curves, Film Negative mode |
| 2 | White Balance & Calibration | 🟡 | Temp & Tint linear matrix | Kelvin scale (800–14,000K), WB Eyedropper, Auto WB, Normalize, Match Look |
| 3 | Exposure, HDR & Tone | ✅ / 🟡 | Exposure (`±4 EV`), Contrast, HDR H/S/W/B | Brightness slider, Auto Adjust, Viewer Exposure/Shadow clip overlays |
| 4 | Levels & Tone Curves | 🟡 | Fritsch-Carlson Luma/R/G/B Curves (`256×1` LUT) | RGB Curve tab, Curve eyedroppers, dedicated Levels tool (In/Out/Auto/Target) |
| 5 | Color Balance, Color Editor & B&W | 🟡 | 3-Way Color Wheels + 8-Band HSL | Master wheel, Advanced arbitrary color range + Uniformity, Skin Tone, B&W 6-channel |
| 6 | Clarity, Dehaze & Vignetting | 🟡 | Spatial Clarity + Optical Vignette | Clarity Methods (Natural/Punch/Neutral/Classic) + Structure, Dehaze, Crop-aware Vignette |
| 7 | Detail, Noise, Grain & Artifacts | 🟡 | Unsharp Sharpen, Chroma/Luma NR, Film Grain | Radius/Threshold/Halo Suppression, AI Enhanced Denoise, 6 Grain types, Moiré, Purple Fringing, Dust Removal |
| 8 | Lens Correction & LCC | ⬜ | — | Lensfun/Manufacturer profiles, CA, Distortion, Diffraction, LCC flat-field |
| 9 | Composition & Geometry | 🟡 | GPU Zoom / Pan / Two-finger Rotate | Interactive Crop + aspect ratios + Grid/Guides, Straighten, Keystone 4-point |
| 10 | Multi-Image Computational | ⬜ | — | HDR Merge (linear DNG), Panorama Stitch, Focus Stacking |
| 11 | Layers, Masks & Local Adjustments | ⬜ | — | Adjustment Layers (up to 16), Brush/Magic Brush, Linear/Radial/Luma Masks, Combine Masks, Heal/Clone |
| 12 | AI Masking & Portrait Retouching | ⬜ | — | Apple Neural Engine (`objc2-vision`) Subject/Background/People masks, Face/Eye/Teeth/Hair Retouch |
| 13 | Sessions, Catalogs, Variants & Stacks | 🟡 | Folder Session catalog, `.rune-edit.json` sidecars, Copy/Paste Look | Full Capture/Selects/Output/Trash Session folders, Virtual Variants, Selective Adjustments Clipboard, Catalogs |
| 14 | Culling, Ratings, Filters & Search | 🟡 | Filmstrip, 0–5★ ratings, Pick/Reject flags, `All/Picks/★3+` filter, `←`/`→` prefetch | 7 Color Tags, Focus Mask, Group Similar, Compare Variant, Advanced Boolean Search |
| 15 | Metadata, Keywords & Naming | ⬜ | Basic file resolution/format HUD | EXIF/IPTC editor, XMP sidecar sync, Hierarchical Keywords, Dynamic Naming Tokens, Batch Rename |
| 16 | Styles, Presets, Speed Edit & Automation | 🟡 | 6 Built-in Styles, native macOS menus & shortcuts | User Styles/Presets, Stacked Styles, Style Brushes, **Speed Edit** (key+drag), Custom Workspaces |
| 17 | Viewer, Scopes & Proofing | 🟡 | Live anti-aliased RGB+Luma Histogram, Before/After Split, 120Hz Metal | Pinned RGB/Lab/HSL Color Readouts, Loupe, Focus Mask, Soft-Proofing, Display P3 & XDR/EDR |
| 18 | Export, Recipes, External Edit & Print | 🟡 | `rayon` parallel 8/16-bit JPEG/PNG/TIFF export (GPU-parity) | Multi-Recipe batch queue, Resizing/Output Sharpening, Watermarks, ICC conversion, Edit With, Contact Sheets, Print |
| 19 | Tethering, Live View, Collaboration & Studio | ⬜ / ➖ | — | Hot Folder watch, USB/Wi-Fi Tethering, Live View, Overlay, LAN/Web Client Review |

> **Where Rune Darkroom Aims to Leapfrog Capture One:**
> - **100% Open Source (MIT)** in Rust—no subscription lock-in, full contributor extensibility.
> - **True Extended Dynamic Range (XDR/EDR) & ISO 21496-1 Gain Map HDR** *(Roadmap Phase 4)* — Capture One still lacks native EDR peak-brightness display rendering and HDR Gain Map export as of 16.9.1.
> - **Ultra-Fast Modern Formats:** Planned native **JPEG XL, AVIF, and WebP** export (none of which Capture One exports as of 16.9.1).
> - **Headless GPU Visual CI (`--chrome --snap`)** + Anomaly OpenCode aesthetic + 120Hz single-pass Metal/wgpu architecture.

---

## 1. Base Characteristics, RAW Engine & Film Negative Conversion

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| RAW Decoding (Multi-Vendor) | Phase One IIQ, Sony ARW, Canon CR2/CR3, Nikon NEF, Fujifilm RAF, Leica/Apple DNG, Panasonic RW2, OM/Olympus ORF, Pentax PEF, etc. | 🟡 | macOS `CIRAWFilter` (ProRAW/JXL-DNG/OS RAWs) + pure-Rust `rawler` linear Bayer fallback. Needs full X-Trans, dual-pixel, and custom AHD/RCD GPU demosaic. |
| Camera Input ICC Profiles | Camera Generic, Generic V2, **ProStandard** profiles, lighting-specific & manufacturer profiles, custom user profiles (`Other`), `No Color Correction` | ⬜ | Currently relies on `CIRAWFilter` or sRGB matrix fallback; needs DCP/ICC camera profile loader (`moxcms`). |
| Film Curves (`Base Characteristics > Curve`) | `Auto`, `Film Standard`, `Film Extra Shadow`, `Film High Contrast`, `Linear Response`, `Linear Scientific` | 🟡 | Single built-in soft filmic highlight roll-off (`filmic_tonemap`); selectable base curves not yet exposed. |
| Fujifilm Film Simulations | Provia, Velvia, Astia, Classic Chrome, Acros, etc. via Curve dropdown | ⬜ | Could be exposed via camera-matched 3D LUTs / profiles. |
| Process Engine Versioning | Legacy engine preservation + one-click Upgrade per variant | 🟡 | `DevelopRecipe.schema_version` exists for future migrations; single engine v1 today. |
| **Film Negative Conversion** *(16.7.4)* | `Processing Mode: Photography vs. Film Negative`, `Convert Negative` button, Film Negative Workspace, remapped Exposure/Levels | ⬜ | High-value community PR candidate (linear inversion + orange-mask neutralization). |
| Native Grayscale Editing *(16.5)* | Native editing of monochrome/grayscale source files | 🟡 | Converted to `Rgba16Float` cleanly; dedicated grayscale output mode not yet present. |
| E-Commerce ICC Profiles *(Studio)* | Specialized product/color-accurate studio profiles | ➖ | Studio/Enterprise specialization. |

---

## 2. White Balance & Color Calibration

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| Temperature & Tint Sliders | Kelvin (`800–14,000 K`) + Tint (Green–Magenta) | 🟡 | Normalized `Temperature` (`-1..+1`) & `Tint` (`-1..+1`) in linear light; not yet mapped to real Kelvin / Bradford CAT02 chromatic adaptation. |
| WB Presets Dropdown | `Shot`, `Auto`, `Daylight`, `Cloudy`, `Shade`, `Flash`, `Tungsten`, `Fluorescent` | ⬜ | Needs EXIF As-Shot WB metadata + standard illuminant presets. |
| Pick White Balance Eyedropper | Click neutral gray/white point on canvas | ⬜ | High-priority Phase 2.5 UX feature. |
| Auto White Balance (`A`) | One-click automatic neutral WB | ⬜ | Gray-world / specular-aware auto WB easy to add on histogram proxy. |
| **Normalize Tool** (`N` / `Shift+N`) | Sample reference color/exposure patch (or enter RGB/HSB) and apply to destination area; Skin Tone presets | ⬜ | Great Capture One signature tool for consistent skin/product matching. |
| **Smart Adjustments** | Reference-based AI Exposure & White Balance matching across batches | ⬜ | Planned after core histogram/exposure statistics. |
| **Match Look** *(16.5)* | AI color/grade transfer from reference image(s) into 4 editable groups (`Normalize`, `Light & Contrast`, `Color Adjust`, `Color Tone`) on Base or Filled Layer | ⬜ | Long-term AI color matching milestone. |

---

## 3. Exposure, High Dynamic Range & Tone Mapping

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| Exposure (`-4.0` to `+4.0 EV`) | Stop-based linear luminance shift | ✅ | Exact `2^EV` linear multiply in `develop.wgsl` + `crates/core` (`-4.0..=+4.0 EV`). |
| Contrast (`-100` to `+100`) | Global midtone contrast | ✅ | 18% middle-gray pivoted contrast curve (`-100..=+100`). |
| Brightness (`-100` to `+100`) | Midtone-weighted luminance shift preserving black/white endpoints | ⬜ | Distinct from Exposure—easy single-pass shader addition. |
| Saturation (`-100` to `+100`) | Intelligent vibrance-like positive saturation + negative desaturation | ✅ | Separate **Vibrance** (skin/chroma-protecting) and **Saturation** sliders shipped. |
| High Dynamic Range (`Highlight`, `Shadow`, `White`, `Black`) | 4 tonal sliders (`-100..+100`) with single-channel highlight reconstruction | 🟡 | All 4 sliders (`Highlights`, `Shadows`, `Whites`, `Blacks`) shipped; RAW single-channel highlight color reconstruction not yet implemented. |
| Auto Adjust (`Cmd+L`) & Configurable Auto | Per-tool `A` button + `Configure Auto Adjustments` checklist | ⬜ | Easy to add using live proxy histogram statistics. |
| Exposure Warning & Shadow Warning Overlays | Configurable red highlight / blue shadow clipping mask directly on Viewer | ⬜ | High-priority GPU shader flag (almost zero cost in `develop.wgsl`). |

---

## 4. Levels & Tone Curves

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Levels Tool** (`RGB`, `R`, `G`, `B`) | Input Shadow / Midtone / Highlight (0–255), Output Shadow / Highlight (0–255), live histogram | ⬜ | High-priority Capture One staple (can bake into existing 256×1 LUT or apply pre-curve). |
| Levels Pickers, Auto Levels & Target Levels | Shadow/Highlight eyedroppers, configurable Auto clipping thresholds (`0.10%`), custom Target Levels | ⬜ | Pairs with Levels tool. |
| **Tone Curve** — `Luma` Channel | Luminance-only spline curve (contrast without saturation shift) | ✅ | Interactive Fritsch-Carlson monotonic cubic spline baked to `256×1` `Rgba16Float` GPU LUT. |
| **Tone Curve** — `Red`, `Green`, `Blue` Channels | Per-channel RGB tone/color balance curves | ✅ | Interactive Luma / R / G / B tabs with draggable knots and ghost curve preview. |
| **Tone Curve** — `RGB` Combined Channel | Traditional RGB curve (boosts saturation alongside contrast) | ⬜ | Easy 5th channel alongside `Luma`. |
| Curve Eyedroppers (`Pick RGB`, `Pick Luma`, `Pick Neutralize Point` *16.7.4*) | Click canvas to place curve point or neutralize cast | ⬜ | High-priority interactive viewport picker. |
| Live Cursor Tonal Indicator on Curve | Orange vertical readout line on curve graph tracking hovered pixel | ⬜ | Great usability polish. |

---

## 5. Color Balance, Color Editor & Black & White

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Color Balance** — `3-Way` (`Shadow`, `Midtone`, `Highlight`) | Side-by-side interactive circular hue/sat wheels + independent Lightness (`Y`) sliders | ✅ | Full interactive 3-Way Color Balance wheels + Luma sliders in `Color` tab & `develop.wgsl`. |
| **Color Balance** — `Master` & Single-Wheel Enlarged Tabs | Global hue/sat Master wheel + enlarged individual Shadow/Midtone/Highlight views | ⬜ | Easy UI tab + uniform addition. |
| **Color Editor — Basic (8-Band HSL)** | 8 color swatches with Hue, Saturation, Lightness sliders | ✅ | 8 bands (Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta) with smooth angular blending. |
| **Color Editor — Direct Cursor Tool (`D`)** | Click-and-drag directly on canvas to tweak Hue/Sat/Lightness of sampled color | ⬜ | High-value Capture One workflow enhancer. |
| **Color Editor — Advanced** *(redesigned 16.9)* | Unlimited custom eyedropper color slices, 2D color wheel with draggable wireframe & visual recoloring, `Smoothness`, **`Uniformity`**, **`Target Picker`**, editable **RGB/HSL target values**, `View Selected Color Range` | ⬜ | Signature Capture One differentiator—major Phase 2.5/3 milestone. |
| **Color Editor — Skin Tone** | Target skin tone picker + `Uniformity` (`Hue`, `Saturation`, `Lightness`) + `Amount` (`H/S/L`) + `Smoothness` | ⬜ | Signature Capture One portrait tool. |
| Color Editor Actions (`Create Masked Layer`, `Save as ICC Profile`) | Convert color range to Adjustment Layer mask; export color grade as custom ICC profile | ⬜ | Depends on Layers & ICC export. |
| **Black & White Tool** | `Enable B&W`, 6-channel `Color Sensitivity` (`Red`, `Yellow`, `Green`, `Cyan`, `Blue`, `Magenta`), `Split Tones` (Highlights & Shadows Hue/Sat) | 🟡 | Monochrome preset exists via `Saturation = -100` + 3-Way wheels; dedicated 6-channel B&W mixer & Split Tones not yet built. |

---

## 6. Clarity, Structure, Dehaze & Vignetting

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Clarity** Slider | Broad midtone local contrast (`-100..+100`) | ✅ | Shipped in `Detail` tab & gated spatial pass in `develop.wgsl`. |
| **Clarity Methods** (`Natural`, `Punch`, `Neutral`, `Classic`) | 4 distinct local-contrast & saturation preservation algorithms | ⬜ | Single clarity algorithm currently; multi-radius pyramid methods planned. |
| **Structure** Slider | Fine micro-contrast texture enhancement / softening | ⬜ | High-priority companion to Clarity (multi-scale DoG/bilateral pass). |
| **Dehaze** (`Amount` + `Auto` / `Pick Haze Shadow Tone`) | Atmospheric haze removal/addition with airlight shadow tone control | ⬜ | Dark-channel prior / airlight GPU pass planned. |
| **Vignetting** (`Amount` `-4..+4 EV` + `Elliptic on Crop`, `Circular on Crop`, `Circular`) | Optical corner darkening/brightening with crop-aware geometry modes | 🟡 | Radial optical vignette (`-1..+1`) shipped; crop-aware elliptic/circular modes await Crop tool. |

---

## 7. Detail, Noise Reduction, Film Grain & Optical Artifacts

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Sharpening** (`Amount`, `Radius`, `Threshold`, `Halo Suppression`) | Full 4-parameter unsharp/deconvolution sharpening | 🟡 | Single `Sharpen` amount slider shipped; needs `Radius`, `Threshold`, and `Halo Suppression`. |
| **Noise Reduction** (`Luminance`, `Details`, `Color`, `Single Pixel`) | Manual RAW noise reduction controls | 🟡 | Single edge-aware `Noise Reduction` slider shipped; needs split Luma/Color/Details/Single-Pixel controls. |
| **AI Enhanced Denoise** *(16.8 / +30% faster 16.9)* | Neural high-ISO Bayer RAW denoise with `Impact` slider | ⬜ | Ideal Apple Neural Engine (`coreml-native`) feature so GPU stays free at 120Hz. |
| **Film Grain** (6 Types + `Impact` & `Granularity`) | `Fine`, `Silver Rich`, `Soft`, `Cubic`, `Tabular`, `Harsh` grain profiles | 🟡 | Single midtone-weighted procedural `Film Grain` amount slider shipped; needs grain size (`Granularity`) & organic simplex/film profiles. |
| **Moiré** (`Amount`, `Pattern`) | False-color interference suppression on fabrics/architecture | ⬜ | Local chroma frequency filter. |
| **Purple Fringing** (`Amount`) | High-contrast edge fringe neutralization | ⬜ | Chromatic edge hue-suppression pass. |
| **Dust Removal / AI Remove Dust** *(16.2+)* | One-click AI sensor dust removal (`Opt+O`) + manual Dust & Spot cursor tools (`O`) | ⬜ | Planned alongside Heal/Clone layers. |

---

## 8. Lens Correction & Lens Cast Calibration (LCC)

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Lens Profiles** (Custom, Manufacturer Embedded, Generic) | Automatic camera/lens EXIF profile matching (incl. Canon RF, Sony, Fuji, M4/3) | ⬜ | Integrate open-source **Lensfun** database + EXIF lens metadata + embedded DNG OpcodeLists. |
| **Chromatic Aberration** (+ `Analyze` button) | Automatic & analyzed lateral CA removal during demosaic/develop | ⬜ | GPU radial R/B channel warp. |
| **Distortion**, **Sharpness Falloff**, **Light Falloff** | Barrel/pincushion, corner softness, and optical vignetting sliders + `Hide Distorted Areas` | ⬜ | Single-pass GPU UV warp + radial gain. |
| **Diffraction Correction** | Aperture-aware optical deconvolution for small f-stops | ⬜ | GPU deconvolution pass. |
| **Lens Movement Tab** | Manual Focal Length, Aperture, Shift X/Y (mm) for technical/tilt-shift lenses | ⬜ | Niche technical-camera feature. |
| **LCC (Lens Cast Calibration)** | Flat-field reference plate profiling for Color Cast, Uniform Light, Sensor Dust, Wide-Angle data | ⬜ | Medium-format/technical camera workflow; clean flat-field division pass if added later. |

---

## 9. Composition & Geometry

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Crop Tool** | Interactive crop handles, presets (`Unconstrained`, `Original`, `Output`, `1:1`, `4:3`, `3:2`, `16:9`, custom), `Switch Orientation` (`Shift+X`), `Crop Outside Image`, auto-orientation sync *(16.8.5)* | ⬜ | **Top Priority (Tier 1)** — essential for daily photography workflow. |
| **AI Crop** *(16.4+)* | Reference-based face/subject crop matching across batches; Studio margin/alignment rules | ⬜ | Can use Apple Vision face/saliency detection on macOS. |
| **Grid & Guides Tools** (`Cmd+G`) | `Rectangular` (Rule of Thirds / N×M), `Golden Ratio`, `Fibonacci Spiral` (mirror/flip), `Follow Crop`, draggable Guides | ⬜ | Clean Rune 2D overlay vector pass over the viewport. |
| **Rotation & Flip** | 90° CW/CCW, fine angle slider, `Straighten` horizon cursor tool, Horizontal/Vertical Flip, **`Auto Rotate`** | 🟡 | Smooth GPU view rotation via trackpad `on_rotate` shipped; needs persistent recipe rotation, 90°/Flip buttons, Straighten horizon tool, and Auto Rotate. |
| **Keystone Correction** (`K`) | `Vertical`, `Horizontal`, `Full` 4-point interactive guides, `Auto`, `Amount` (80% default), `Aspect` (`-50..+100` incl. anamorphic), `Skew`, Keystone Loupe *(16.9.1)* | ⬜ | Clean GPU 3×3 homography transform in `develop.wgsl`. |

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
| **Adjustment Layers** (up to 16) | `Image Layer` (base) + `Empty` / `Filled` Adjustment Layers, Opacity (`0–100%`), visibility, reorder, rename, Duplicate Layer | ⬜ | **Phase 3 Core Milestone** — reuse `DevelopRecipe` per masked layer. |
| **Manual Brush (`B`) & Eraser (`E`)** | Size, Hardness, Opacity, Flow, Airbrush, **Auto Mask** (edge-aware), Pen Pressure, Link Brush/Eraser | ⬜ | Sparse `256×256` CoW tile mask engine + GPU dab splatting (note: stylus pressure needs Rune pen API). |
| **Magic Brush & Magic Eraser** | Color/tone/texture-aware smart painting (`Tolerance`, `Refine Edge`, `Sample Entire Photo`) | ⬜ | GPU flood/edge-aware region growing. |
| **Linear (`L`) & Radial (`T`) Gradient Masks** | Parametric 3-line / 3-ring controls, rotate/asymmetric feather, Invert, Rasterize | ⬜ | Analytic SDF evaluation directly in WGSL (zero mask RAM until rasterized!). |
| **Luma Range Mask** | Luminosity ramp with fall-off handles, `Radius`, `Sensitivity`, `Invert Range` | ⬜ | Real-time luminance-key evaluation in WGSL. |
| **Combine Masks** *(16.7+)* | Non-destructive **Add**, **Subtract**, **Intersect** boolean mask stacks inside a single layer | ⬜ | Clean mask-graph composition in Phase 3. |
| **Mask Operations & Display Modes** | Copy Mask From, Invert, Clear, Fill, Feather, Refine, Rasterize; Display Always / While Drawing / Never / Grayscale (`Opt+M`); Export Masks to PSD | ⬜ | Pairs with Layers & Masks panel. |
| **Heal (`Q`) & Clone (`S`) Layers** | Dedicated repair layers, multi-stroke Auto Source Point healing, manual draggable source markers | ⬜ | Phase 3 retouching milestone. |
| **Style Brushes** | One-stroke masked layer creation with bundled local adjustments | ⬜ | Pairs with Layers + Styles. |

---

## 12. AI Masking & AI Portrait Retouching Suite

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **AI Subject & Background Masking** *(16.3+)* | One-click primary Subject / Background masks; dynamically recomputes in Styles/Batch | ⬜ | **Phase 3 Mac-First Superpower:** `objc2-vision` (`VNGenerateForegroundInstanceMaskRequest`) on Apple Neural Engine via zero-copy `IOSurface`! |
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
| **4-Folder Session Structure & Templates** | Capture / Selects (`⌘J`) / Output / Trash folders, Session Favorites, Session Albums/Smart Albums, Session Templates, Session Builder *(16.6+)* | ⬜ | Great Tier 2 upgrade to our folder-first Session workflow. |
| **Centralized Catalogs** | Managed vs. Referenced SQLite Catalogs, Projects, Groups, Albums, Smart Albums, Offline Previews, Locate/Sync/Verify/Backup, Lightroom/Aperture Import | ⬜ | Planned as optional catalog index over SQLite after Sessions mature. |
| **Virtual Variants (`New Variant`, `Clone Variant`)** | Multiple non-destructive edit versions pointing to 1 source RAW | ⬜ | High-value Capture One feature (store multiple variants in `.rune-edit.json` array!). |
| **Variant Stacks, Primary vs. Selected & Multi-Edit** | Thick/thin selection borders, `Edit All Selected Variants` toggle, Promote/Demote stack order | ⬜ | Enables true Capture One batch editing. |
| **Adjustments Clipboard (Selective Per-Tool Copy/Apply)** | Granular checkbox list per tool + Autoselect Adjusted + Additive Layer sync | 🟡 | Full-recipe `Copy Look` (`⌘⇧C`) / `Paste Look` (`⌘⇧V`) shipped; needs selective per-tool checklist UI. |
| **Enhanced Image Package (`.eip`)** | Self-contained RAW + edits + ICC archive | ⬜ | Optional archive export later. |

---

## 14. Culling, Ratings, Filtering & Search

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Filmstrip / Browser Views** | Bottom Filmstrip, Grid View, List View, Secondary Display | 🟡 | Collapsible bottom Filmstrip (`F`) with fast thumbnails + background neighbor prefetching shipped; needs Grid View & Secondary Display. |
| **Star Ratings (`0–5`)** | Keyboard shortcuts `0`–`5` + clickable thumbnail stars | ✅ | Shipped (`0`–`5` shortcuts + interactive star bar on filmstrip cards). |
| **Pick / Reject & 7 Color Tags** | Pick/Selects + Red/Orange/Yellow/Green/Blue/Purple/Pink tags + Auto-Advance | 🟡 | Pick (`P`) / Reject (`X`) flags shipped; needs 7 Color Tags & Auto-Advance on rate/tag. |
| **Filters Tool & Search** | Quick filter bar + faceted counts (Camera, Lens, ISO, Date, Orientation, Tag, Rating) + Boolean Advanced Search | 🟡 | `All | Picks | ★3+` filter segmented control shipped; needs EXIF/metadata faceted filters & search. |
| **Dedicated Cull View & Group Similar** | Instant preview culling, AI burst similarity grouping with slider, Automatic Face Zoom | ⬜ | Huge workflow win for wedding/event/portrait photographers. |
| **Assisted Review (AI Culling)** *(16.8 / 16.9)* | Auto-flags closed eyes, out-of-focus eyes, blank frames; per-face Viewer indicator | ⬜ | Can leverage Apple Vision face/eye-landmark + sharpness scoring on macOS. |
| **Compare Variant & Multi-View** | Pin orange-bordered reference variant side-by-side against cycling candidates; multi-image viewer | ⬜ | High-priority Capture One signature culling feature. |
| **Focus Mask Overlay** | Real-time acutance highlight overlay in Viewer & Browser | ⬜ | Fast Sobel/Laplacian GPU shader overlay! |

---

## 15. Metadata, Keywords, Naming Tokens & Ingest

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **EXIF / IPTC / GPS Metadata Panel** | Read camera/lens EXIF, edit IPTC (Caption, Creator, Copyright, Getty fields), batch copy metadata | ⬜ | **Tier 1/2 Priority** — read EXIF (camera, lens, f-stop, shutter, ISO, focal length) right into status bar & Inspector! |
| **XMP Sidecar Sync** | 2-way Full Sync / Load / Manual `.xmp` sidecar metadata & crop/rotation interop | ⬜ | Important for interoperability with Lightroom/Photo Mechanic. |
| **Keywords & Hierarchical Keyword Libraries** | Parent/child `|` trees, `.txt` import/export, export filter toggles | ⬜ | Pairs with Catalog/Session search. |
| **Metadata Presets** | Reusable copyright/creator templates applied on import/tether | ⬜ | Pairs with Metadata panel. |
| **Dynamic Naming Tokens & Batch Rename** | Pill tokens (Counters, Date, EXIF, IPTC, Job/Collection/Original Name) across Import, Rename, Export, Watermarks | ⬜ | Essential for pro batch export & file organization. |
| **Import Dialog** | Card ingest, token subfolders, dual-drive `Backup To`, duplicate exclusion, Style on import, eject | ⬜ | Folder-first workflow skips mandatory import, but an optional SD-card ingest dialog is very helpful. |
| **C2PA Content Credentials** *(16.9.1)* | Cryptographic provenance signing on JPEG/TIFF export | ⬜ | Open-source `c2pa-rs` crate makes this surprisingly clean later! |
| **Sequences** | Sequence IDs, dynamic sequence subfolders on import/tether/export | ➖ | Specialized studio/focus-stack workflow. |

---

## 16. Styles, Presets, Speed Edit, Customization & Automation

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Built-in & Custom Styles** | Built-in looks + Save User Style (selective tools) + `.costyle` / `.costylepack` import | 🟡 | 6 curated Capture One-grade built-in Styles (`Neutral`, `Warm Film`, `Classic Chrome`, `Editorial B&W`, `Moody Dusk`, `Alpine Crisp`) shipped; needs user-saved Styles & `.json` pack import/export. |
| **Stacked Styles & Per-Tool Presets** | Layer multiple Styles cumulatively; per-tool preset dropdowns (e.g., Curve presets, WB presets) | ⬜ | High-value workflow multiplier. |
| **Speed Edit** (Key + Drag / Scroll) | Hold hotkey (`Q`=Exposure, `W`=Contrast, `A`=Highlights, `S`=Shadows, etc.) + drag/scroll anywhere on screen | ⬜ | **Top Capture One UX Superpower** — super clean to implement in Rune's `on_key`/`on_wheel`/`on_drag`! |
| **Custom Workspaces, Tool Tabs & Floating Tools** | Reorder/detach panels, custom tool tabs, save/switch layouts | 🟡 | Collapsible Filmstrip (`F`) & Inspector (`I`) + 4 clean OpenCode tabs shipped; full panel customization later. |
| **Custom Keyboard Shortcuts** | User-editable shortcut sets + HTML cheat-sheet export | 🟡 | Rich macOS default shortcuts + native `NSMenu` key equivalents shipped; remappable shortcut UI (`rune-kit::ShortcutRecorder`!) ready to wire! |
| **Tangent Grading Panels** | Hardware colorist control surface support | ➖ | Niche hardware integration. |
| **AppleScript / Background Scripts / Plugin SDK** | Event hooks (`CO_CaptureDone`, `CO_BatchJobFinished`), AppleScript dictionary, C++/plugin SDK | ⬜ | Future Lua/WASM or CLI automation hooks. |

---

## 17. Viewer, Scopes, Proofing & Display Color Management

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Live RGB + Luma Histogram** | Real-time tonal distribution + shadow/highlight clipping telemetry | ✅ | Anti-aliased RGB + Luma area histogram with `BLK %` / `WHT %` clip readouts pinned in inspector. |
| **RAW Exposure Evaluation Meter** | Pre-adjustment sensor histogram + $\pm 2\text{ EV}$ meter | ⬜ | Useful once RAW sensor stats are exposed. |
| **Before / After (Split & Full View)** | Draggable split divider + instant momentary full-canvas Original toggle (`\`) | ✅ | Crisp GPU split divider with frosted HUD badges + `\` toggle shipped. |
| **Zoom, Pan & Smooth Canvas** | Fit, 100%, 200%, pinch-to-zoom, pan, rotate | ✅ | Locked 120Hz Metal GPU viewport (`Fit | 150% | 200%`), native trackpad pinch (`on_magnify`), rotate (`on_rotate`), two-finger double-tap (`on_smart_magnify`), wheel/drag pan. |
| **Color Readouts (RGB / Lab / CMYK / HSL)** *(HSL added 16.7)* | Live cursor pixel probe + multiple pinned Viewer readout markers | ⬜ | **High-Priority Tier 1** — trivial to sample from `LinearImage` + `CompiledRecipe` under pointer! |
| **Loupe Tool** | High-magnification floating pixel inspector in Viewer & Browser | ⬜ | Great GPU shader circle-magnifier overlay. |
| **Annotations Tool** | Freehand sketch notes over Viewer, exportable as PSD layer | ⬜ | Studio retoucher communication feature. |
| **Proof Margin & Slideshow** | Configurable mat margin & full-screen slideshow mode | 🟡 | Calibrated neutral `#121212` matte margin shipped; configurable margin/slideshow later. |
| **ICC Soft-Proofing & Recipe Proofing** | Real-time Viewer simulation of target ICC profile, output scale, sharpening, watermark, and JPEG compression | ⬜ | Requires `moxcms` / ColorSync output profile transform. |
| **Wide-Gamut Display P3 / ColorSync** | Native P3 monitor color management | ⬜ | **Phase 4 Milestone** — internal pipeline is already scene-referred linear `Rgba16Float`; awaits P3/EDR swapchain in Rune (or patched `MetalSurface`). |
| **Native XDR / EDR HDR Display & Gain Maps** | >1.0 peak brightness rendering on Apple XDR displays + ISO 21496-1 Gain Maps | ⬜ | **Differentiator Opportunity** — Capture One *still* does not support EDR display or Gain Map HDR as of 16.9.1! |

---

## 18. Export, Output Recipes, External Editing, Contact Sheets & Print

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Single Image Export (JPEG, PNG 8/16, TIFF 16)** | Full-resolution graded export matching Viewer color science | ✅ | `rayon`-parallel export (`⌘⇧E`) to JPEG, 8-bit/16-bit PNG, and 16-bit TIFF with verified pre-tonemap GPU parity! |
| **Simultaneous Multi-Recipe Export Queue** | Batch-check multiple Export Recipes (e.g., Full TIFF + Web JPEG + Proof) in one pass with background queue | ⬜ | **High-Priority Tier 2** — signature Capture One productivity feature. |
| **Additional Export Formats** | PSD (8/16-bit), JPEG QuickProof, JPEG 2000, DNG, Originals + EIP *(Note: C1 lacks WebP/AVIF/JXL export)* | 🟡 | We can surpass C1 easily by adding **WebP, AVIF, and 16-bit JPEG XL** via `ImageIO` / Rust codecs! |
| **Resize / Scaling Modes & Output Sharpening** | Fixed %, Width, Height, Long/Short Edge, Dimensions + Screen & Print viewing-distance Output Sharpening | ⬜ | High-priority Export Dialog upgrade. |
| **Output ICC Profile Conversion & Metadata Strip** | Convert to sRGB / Display P3 / Adobe RGB / CMYK; toggle EXIF/GPS/Copyright/Keywords | ⬜ | Straightforward via `moxcms` + metadata writer. |
| **Text & Image Watermarking** | Token-based Text or PNG logo watermark with scale, opacity, X/Y offset | ⬜ | Clean GPU or export-stage composite pass. |
| **External Editing (`Edit With` / `Open With`)** | Round-trip TIFF/PSD/Affinity `.af` to Photoshop or Affinity Photo with auto re-import | ⬜ | macOS `NSWorkspace` launch + file-watcher reload. |
| **Contact Sheets** *(overhauled 16.7+)* | Branded multi-page PDF/JPEG contact sheets with cover, header/footer, ratings, and color tags | ⬜ | Nice Tier 3/4 export module. |
| **Print Engine** | Print templates, grid layout, margins, printer ICC profiles, rendering intent, BPC, print sharpening | ⬜ | Lower priority for digital-first MVP. |

---

## 19. Tethering, Live View, Studio Production, Collaboration & Mobile

| Capture One Feature | Details / Controls | Rune Darkroom | Notes |
| :--- | :--- | :---: | :--- |
| **Hot Folder Watch (Auto-Ingest)** | Automatically display & grade new images dropped into a watched folder | ⬜ | Lightweight, universally compatible way to support tethering from any camera/SD-card tool! |
| **Wired & Wireless Camera Tethering** | Direct USB/Wi-Fi/Ethernet SDK control for Canon, Nikon, Sony, Fujifilm, Leica, Panasonic, Phase One, Hasselblad + **ReTether** + **2nd Gen Canon Wireless** | ➖ | Requires proprietary camera SDKs (`EDSDK`, `CrSDK`, `Nikon MAID`) or `libgphoto2`—major separate plugin track later. |
| **Live View, Camera Focus & Focus Meter** | Real-time sensor stream, remote AF/focus nudging, 3-box contrast Focus Meter, Composition Mode | ➖ | Tied to camera tether SDKs. |
| **Overlay Tool** | Semi-transparent PNG/PSD/TIFF art-direction mockup over Viewer & Live View | ⬜ | Easy standalone Viewer overlay even without tethering! |
| **Capture One Live / Live for Studio / Multi-User Sessions** *(16.9 GA)* | Cloud web client review, offline LAN iPad review, real-time LAN multi-workstation Session editing | ➖ | Cloud/LAN studio infrastructure—out of scope for v1. |
| **Enterprise & Cultural Heritage (CH)** | Jamf Pro deployment, Admin Portal, Tool Locks + PIN, Barcode/QR scanner, Copy Stand, Slipstream, PPI Assist, AutoColumn | ➖ | Specialized Enterprise/Museum editions. |
| **Capture One Mobile (iPad / iPhone) & Cloud Transfer** | Mobile RAW editing/tethering + Cloud album transfer | ➖ | Future cross-platform desktop (Windows/Linux) comes first; Rune iOS exists long-term. |

---

## Recommended Next Implementation Priorities (PR Roadmap)

If you want to close the highest-impact gaps for real-world photographers fastest (and great places for community PRs!):

### Tier 1 — Daily-Driver Photographer Essentials
1. **Interactive Crop, Straighten & Composition Grid (`Rule of Thirds` / `Golden Ratio`)** + persistent Rotation/Flip in `DevelopRecipe` & `develop.wgsl`.
2. **EXIF / Camera Metadata Readout** (Camera model, Lens, Focal Length, Aperture, Shutter, ISO) in the Status Bar & Inspector.
3. **White Balance Eyedropper + Kelvin Scale + Auto WB / Auto Exposure**.
4. **Speed Edit (Key + Drag / Scroll)** (`Q` Exposure, `W` Contrast, `A` Highlights, `S` Shadows, `1`/`2` Temp/Tint).
5. **Viewer Overlays & Probes:** Exposure/Shadow Clipping Warning overlay, **Focus Mask**, and **Live RGB / HSL / Lab Color Readout** under cursor.

### Tier 2 — Signature Capture One Color & Workflow Depth
6. **Levels Tool** (`RGB`, `R`, `G`, `B` Input/Output + Auto + Pickers) + **Brightness** + **Structure** + **Dehaze** + **6-Channel Black & White**.
7. **Advanced Color Editor & Skin Tone Uniformity** (arbitrary color range selection, `View Selected Color Range`, Skin Tone Uniformity).
8. **Virtual Variants** (`New Variant` / `Clone Variant`), **Compare Variant** (pinned orange reference), **7 Color Tags**, and **Selective Adjustments Clipboard**.
9. **Export Dialog Upgrades:** Resize/Long-Edge scaling, Output Sharpening, ICC color space conversion, Text/Image Watermark, Multi-Recipe batch export, plus **WebP / AVIF / JPEG XL**.
10. **Lens Correction** (Lensfun + Distortion + Chromatic Aberration + Vignette) & **Keystone** 4-point perspective.

### Tier 3 — Local Adjustments, Masks & Apple Neural Engine AI
11. **Layers & Masks Engine:** Non-destructive Adjustment Layers, Linear/Radial Gradient SDF masks, Luma Range masks, Sparse `256×256` CoW Brush/Eraser, Combine Masks (`Add`/`Subtract`/`Intersect`), and Heal/Clone.
12. **Apple Neural Engine AI (Zero-Copy `IOSurface`):** One-click **AI Subject / Background / People Masking** (`objc2-vision`), **AI Dust Removal**, and **AI Denoise** (`coreml-native`) while keeping the GPU 100% dedicated to 120Hz interaction.

### Tier 4 — Leapfrog Features & Cross-Platform
13. **Wide-Gamut Display P3 + Native XDR/EDR HDR Display** + **ISO 21496-1 Adaptive Gain Map** export (beating Capture One 16.9!).
14. **Film Negative Conversion Mode** (camera-scanned film negative inversion + mask neutralization).
15. **Hot Folder Auto-Ingest**, **Windows (DX12) & Linux (Vulkan) CI**, and GPU WGSL AHD/RCD demosaic.

---

## Sources
- Capture One Official Help Center (`https://support.captureone.com/hc/en-us` — live index of 10 categories, 84 sections, 1,299 published articles as of Nov 25, 2026)
- Capture One Desktop Release Notes (16.2 through **16.9.1**, `https://support.captureone.com/hc/en-us/sections/360000331817-Capture-One-desktop`)
- Capture One Product Features (`https://www.captureone.com/en/features`)
