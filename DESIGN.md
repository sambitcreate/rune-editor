---
version: "1.0"
status: "Locked v1 — Studio Airy (Applied)"
preset: "studio_airy"
name: "Rune Darkroom"
platform: "macOS (Primary, Apple HIG) · Cross-Platform Rust / Rune GPU UI"
theme_base: "Anomaly OpenCode Dark (rune-theme-opencode v2)"
description: "Neutral, color-accurate darkroom workspace pairing Anomaly OpenCode monochrome precision with Capture One Pro studio ergonomics, macOS System Settings inheritance, and 3-tier progressive disclosure."

colors:
  # Core Neutral Surface Ladder (zero hue cast so color grading stays objective)
  bg-deep: "#080808"          # Titlebar, histogram/curve/levels wells, tab bar track, code blocks
  page: "#121212"             # Main viewport backdrop behind the photograph
  bg-base: "#161616"          # Inspector rail, scrubbable capsule tracks, settings/export cards, Studio proof mat
  bg-exhibition-mat: "#1E1E1E" # Exhibition proof margin gallery mat
  layer-01: "#242424"         # Collapsible tool cards, elevated panels
  layer-02: "#2E2E2E"         # Capsule hover, navigation item hover
  layer-03: "#3A3A3A"         # Active segmented pill / selected layer / active tab surface
  layer-04: "#5C5C5C"         # High-contrast neutral fill

  # Text & Icon Hierarchy
  text-base: "#FAFAFA"        # Primary labels, modified values, active titles
  text-muted: "#AEAEAE"       # Rest control labels, inactive tabs, secondary metadata
  text-faint: "#808080"       # Hints, neutral counts, unit suffixes, disabled cues
  text-inverse: "#161616"     # Text on contrast buttons
  text-accent: "#A2BCFF"      # Active selection text, modified tool dot, keyboard focus

  # Hairlines, Dividers & Interactive Overlays
  border-muted: "rgba(255, 255, 255, 0.08)"
  border-base: "rgba(255, 255, 255, 0.10)"
  border-strong: "rgba(255, 255, 255, 0.20)"
  border-focus: "#A2BCFF"
  overlay-hover: "rgba(255, 255, 255, 0.06)"
  overlay-pressed: "rgba(255, 255, 255, 0.10)"
  capsule-fill: "rgba(255, 255, 255, 0.13)"
  capsule-edge: "rgba(255, 255, 255, 0.24)"
  capsule-zero-tick: "rgba(255, 255, 255, 0.10)"

  # Semantic & Darkroom Functional Accents (used strictly for photographic state, never chrome decoration)
  accent-interactive: "#3B5CF6"
  info: "#7698FD"             # Shadow clipping telemetry, info highlights
  danger: "#F17471"           # Highlight clipping telemetry, destructive reset
  success: "#6BD586"          # Passing checks, active MCP / watcher indicator
  warning: "#F2CF76"          # Non-fatal warnings
  darkroom-amber: "#F58F29"   # Capture One tonal probe, modified channel bars, mask guides, heal target, pinned readouts
  compare-orange: "#E87A2A"   # Compare Variant reference frame border
  film-c41: "#F28F3D"         # Film Negative mode indicator & customized shortcut cue
  mask-rubylith: "#F04D4D"    # Red Rubylith layer mask overlay HUD
  clone-cyan: "#52C7F5"       # Clone Stamp target ring
  mcp-green: "#47D67A"        # Local MCP server running status dot

typography:
  families:
    sans: "'Inter', -apple-system, BlinkMacSystemFont, 'SF Pro Text', sans-serif"
    mono: "'IBM Plex Mono', 'SF Mono', ui-monospace, monospace"
  features:
    numeric: "'tnum' 1"
  scale:
    dialog-title: { size: "15px", weight: 530, lineHeight: 1.50, tracking: "-0.01em" }
    brand-title:  { size: "13px", weight: 530, lineHeight: 1.50, tracking: "-0.005em" }
    body:         { size: "13px", weight: 400, lineHeight: 1.50, tracking: "0em" }
    control:      { size: "12.5px", weight: 440, lineHeight: 1.50, tracking: "0em" }
    control-med:  { size: "12.5px", weight: 530, lineHeight: 1.50, tracking: "0em" }
    section:      { size: "11.5px", weight: 600, lineHeight: 1.50, tracking: "+0.06em", transform: "uppercase" }
    caption:      { size: "12px", weight: 400, lineHeight: 1.50, tracking: "0em" }
    mono-value:   { size: "12px", weight: 400, lineHeight: 1.50, features: "'tnum' 1" }
    mono-compact: { size: "11px", weight: 400, lineHeight: 1.50, features: "'tnum' 1" }
    mono-probe:   { size: "10.5px", weight: 400, lineHeight: 1.50, features: "'tnum' 1" }
  locked:
    base-font: "12.5px"
    line-height: 1.50
    section-tracking: "+0.06em"

spacing:
  base: "4pt"
  scale: [2, 4, 6, 8, 10, 12, 14, 16, 20, 24]
  inspector-outer-pad: "14pt"
  card-pad: "14pt"
  card-gap: "12pt"
  card-header-gap: "10pt"
  group-gap: "14pt"
  row-gap: "8pt"
  control-gap: "8pt"
  pill-gap: "6pt"

sizing:
  window-default: [1360, 860]
  window-min: [1024, 680]
  titlebar-h: "50pt"
  status-bar-h: "30pt"
  inspector-w: "388pt"        # Recommended range: 380–396pt (was 356pt)
  card-inner-w: "332pt"       # 388 - (14*2 outer) - (14*2 card) (was 308pt)
  histogram-h: "72pt"         # (was 64pt)
  capsule-slider-h: "32pt"    # Recommended range: 32–34pt (was 30pt)
  mini-capsule-h: "24pt"      # (was 22pt)
  pill-h: "28pt"              # (was 24pt)
  pill-compact-h: "26pt"      # (was 22pt)
  action-pill-h: "30pt"       # (was 28pt)
  info-badge-size: "14pt"     # Tier 2 hover ⓘ icon diameter
  info-popover-max-w: "272pt"

rounded:
  xs: "2pt"
  sm: "4pt"
  md: "6pt"
  lg: "8pt"
  xl: "10pt"
  pill: "9999pt"

shadows:
  popover: "0 12px 32px rgba(0, 0, 0, 0.56), 0 0 0 0.5px rgba(255, 255, 255, 0.14)"
  hud-float: "0 8px 24px rgba(0, 0, 0, 0.48), 0 0 0 0.5px rgba(255, 255, 255, 0.12)"
  puck: "0 1px 3px rgba(0, 0, 0, 0.60)"

progressive_disclosure:
  tier_0_max_visible_controls_per_card: 5
  tier_0_max_expanded_cards_per_tab: 3
  tier_1_disclosure_label: "Advanced"
  tier_1_auto_badge_when_modified: true
  tier_2_info_badge:
    diameter: "14pt"
    default_mode: "reveal-on-hover" # Options: subtle-always | reveal-on-hover | off
    hover_delay_ms: 180
    dismiss_delay_ms: 120
    fields: ["explanation", "shortcut", "cli_mcp_param"]

navigation:
  titlebar_concept: "Concept A — Minimal Pro Titlebar + Overlays Popover"
  native_menus: ["File", "Edit", "Image", "View", "Window"] # + Rune Darkroom app menu
  max_top_level_rows_per_menu: 8

settings_inheritance:
  preset: "studio_airy"
  card_style: "macos-system-settings-grouped"
  card_pad: "14pt"
  card_gap: "12pt"
  row_gap: "8pt"
  group_gap: "14pt"
  row_divider: "1px rgba(255, 255, 255, 0.08)"
  line_height: 1.50
  sidebar_w: "232pt"
  content_max_w: "740pt"
  categories: ["Agent & Local MCP", "CLI & Agent Skill", "Shortcuts & Workflow", "Security & Storage"]
  tier_1_disclosures:
    - "Connection Config Snippets"
    - "Exposed MCP Tools (9)"
    - "CLI Workflow Examples"
    - "Workflow Shortcuts"
    - "Security & Sandbox Invariants"
  tier_2_info_labels:
    - "Local MCP Server"
    - "Allow Live Canvas Edits"
    - "Port Binding"
    - "Agent Snippets"
    - "Hot Folder Watch"
    - "Proof Margin"
    - "Custom Shortcuts"
    - "Inviolable Originals"
---

# Rune Darkroom — Design System Specification (`DESIGN.md`)

> **Status:** `Locked v1 — Studio Airy (Applied)`  
> **Scope:** Authoritative visual, ergonomic, progressive-disclosure, Settings inheritance, and navigation standard for Rune Darkroom (`crates/app`), grounded in the live codebase (`crates/app/src/{tokens.rs,menu.rs,toolbar.rs,widgets.rs,layers_panel.rs,settings_view.rs,export_view.rs}`) and `rune-theme-opencode`.

### Locked `Studio Airy` Core Metrics

| Token / Rule | Locked Value | Applies To |
| :--- | :--- | :--- |
| `INSPECTOR_W` / `CARD_INNER_W` | **`388pt`** / **`332pt`** | Right develop inspector rail & inner card track |
| `CAPSULE_SLIDER_H` | **`32pt`** | Monochrome scrubbable capsules & inline selectors |
| `CARD_PAD` / `CARD_GAP` | **`14pt`** / **`12pt`** | Inspector tool cards **and** Settings grouped cards |
| `ROW_GAP` / `GROUP_GAP` | **`8pt`** / **`14pt`** | Control rows, setting rows, and logical groups |
| `BASE_FONT` / `LINE_HEIGHT` / `SECTION_TRACKING` | **`12.5px`** / **`1.50×`** / **`+0.06em`** | All chrome labels, captions, popovers, and uppercase headers |
| Hover `ⓘ` Info Mode | **`Reveal on Hover`** (`180ms` intent delay) | Inspector headers/sliders **and** all Settings rows |
| Titlebar Layout | **Concept A — Minimal Pro Titlebar + `Overlays ▾` Popover** | Main workspace & Settings toggle (`Settings` / `MCP On`) |
| Native macOS Menu Bar | **5 scannable menus (`≤ 8` rows each)** | `File`, `Edit`, `Image`, `View`, `Window` (+ `Rune Darkroom`) |

---

## 1. Visual Theme & Atmosphere

Rune Darkroom is a **neutral, color-calibrated studio darkroom** for photographers and AI-assisted editing workflows. Its visual atmosphere blends three disciplines:

1. **Anomaly OpenCode Monochrome Restraint:** Chromatic neutrality across every piece of UI chrome (`#080808` → `#161616` → `#242424`) so human eyes judge skin tones, white balance, and shadow roll-off against an unbiased surround.
2. **Capture One Pro Ergonomics:** Scrubbable capsule sliders without protruding knobs, live RGB/Lab pixel telemetry, warm-amber photographic indicators (`#F58F29`), and direct-manipulation curves, wheels, and masks.
3. **macOS Tahoe Human Interface Craft:** Generous breathing room, crisp hairline elevation, scannable native menus (`≤ 8` top-level rows per menu), and **3-tier progressive disclosure** so the workspace starts calm and reveals studio-grade precision only when requested.

### Audit Finding — Why the Current UI Feels Crowded

Direct inspection of `crates/app/src/` reveals five concrete sources of visual pressure:

| Audit Area | Current Codebase State | Ergonomic Impact |
| :--- | :--- | :--- |
| **Default Card Expansion (`state.rs`)** | **25 of 28** collapsible cards default to `open: true` (all 5 on `Detail`, all 5 on `Lens`, all 4 on `Layers`, all 4 on `Style`). | Every tab dumps **21–37 controls** at once, forcing long scrolling before reaching core tools. |
| **Cramped Line-Heights (`tokens.rs` + `opencode`)** | `ty::section_header` & `ty::caption` use `11px / 12px` (**`1.09×` line-height**); `mono_status` is `11px / 14px` (`1.27×`); `mono_probe` is `9.5px`. | Wrapped helper text nearly collides vertically; dense rows feel compressed on Retina displays. |
| **Tight Inspector Rhythm (`tokens.rs`, `widgets.rs`)** | `INSPECTOR_W = 356pt`, outer pad `10pt`, card pad `10pt`, card gap `8pt`, control gap `5pt`, compact pills `18–22pt`. | Wall-to-wall packed pills with weak visual distinction between Card, Group, and Control. |
| **Overloaded Titlebar (`toolbar.rs`)** | **16 interactive controls** (18 click targets) + 5 dividers + brand + filename + badge inside one `48pt` bar. | Diagnostic overlays (`Clipping`, `Focus`, `Mask`, `Loupe`) compete with primary actions and squeeze filenames at `1024pt`. |
| **Flat 15-Item View Menu (`menu.rs`)** | `View` has **15 flat items**; `Edit` packs 5 photo Variant items; shortcut mismatches (`Loupe` says `⌥L` in `menu.rs` vs `O` in `toolbar.rs`; `Filmstrip`/`Inspector` say `⌘⌥F`/`⌘⌥I` vs `F`/`I`). | Slow keyboard discovery and ungrouped diagnostic/workflow toggles. |

---

## 2. Color Palette & Roles

All UI chrome MUST stay strictly achromatic so it never induces simultaneous contrast shifts on the photograph. Color enters the interface **only** for photographic telemetry, active selection focus, or semantic state.

### 2.1 Neutral Surface Ladder

| Token (`tokens.rs` / `Kit`) | Hex / RGBA | Functional Role |
| :--- | :--- | :--- |
| `k.c.bg_deep` | `#080808` | Recessed wells: titlebar, histogram & curve plots, tab bar track, code blocks, nested mask sub-cards |
| `k.c.page` | `#121212` | Main canvas backdrop behind the photo viewport & full-page Settings/Export workspaces |
| `k.c.bg_base` | `#161616` | Right inspector rail, Scrubbable Capsule tracks, Settings/Export cards, Studio Proof Margin mat |
| `color::proof_margin_exhibition` | `#1E1E1E` | Exhibition Proof Margin gallery mat (`70pt` outer surround) |
| `k.c.layer_01` | `#242424` | Collapsible tool cards inside the inspector, active recipe cards |
| `k.c.layer_02` | `#2E2E2E` | Capsule hover fill, sidebar nav hover |
| `k.c.layer_03` | `#3A3A3A` | Active inspector tab, selected pill/radio, selected layer row, frosted popover base |
| `k.c.layer_04` / `bg_contrast` | `#5C5C5C` | High-contrast neutral primary CTA fills |

### 2.2 Text, Borders & Capsule Fill

| Token | Value | Role |
| :--- | :--- | :--- |
| `k.c.text_base` | `#FAFAFA` | Active titles, modified slider labels & values, primary headings |
| `k.c.text_muted` | `#AEAEAE` | Neutral/rest slider labels, inactive tabs, secondary labels, `ⓘ` icon at rest |
| `k.c.text_faint` | `#808080` | Section counters (`2/8`), inactive badges, subtle unit labels |
| `k.c.text_accent` / `border_focus` | `#A2BCFF` | Active pill text, modified tool indicator dot (`4pt`), keyboard focus ring (`2pt`) |
| `k.c.border_muted` | `rgba(255,255,255,0.08)` | Internal header hairlines (`0.5pt`), sub-group dividers, nested sub-card outlines |
| `k.c.border_base` | `rgba(255,255,255,0.10)` | Card outlines, capsule borders, toolbar/inspector edge dividers |
| `k.c.border_strong` | `rgba(255,255,255,0.20)` | Active tab/pill borders, active popover perimeter |
| `color::capsule_fill` | `rgba(255,255,255,0.13)` | Monochrome progress fill inside Scrubbable Capsules |
| `color::capsule_edge` | `rgba(255,255,255,0.24)` | `1pt` leading value indicator edge inside Scrubbable Capsules |
| `color::capsule_zero_tick` | `rgba(255,255,255,0.10)` | Center magnetic zero-detent tick on bipolar capsules |

### 2.3 Calibrated Darkroom Functional Accents

| Semantic Token | Hex | Strict Usage Boundary |
| :--- | :--- | :--- |
| `darkroom-amber` (`curve_tonal_indicator`, `levels_modified_bar`, `mask_guide_primary`, `heal_target_ring`, `pin_marker_ring`) | `#F58F29` | Live tonal probe on curve, modified per-channel Levels/Color bars, Linear/Radial mask vector guides, Heal rings, pinned color pins |
| `compare-orange` (`compare_reference_border`) | `#E87A2A` | Compare Variant reference border & badge |
| `film-c41` (`film_negative_accent`) | `#F28F3D` | Film Negative processing mode indicator & customized shortcut highlight |
| `mask-rubylith` (`mask_overlay_red`) | `#F04D4D` | Red Rubylith mask display HUD & active toggle text |
| `clone-cyan` (`clone_target_ring`) | `#52C7F5` | Clone Stamp canvas target ring |
| `mcp-green` (`mcp_running`) | `#47D67A` | Local MCP server & Hot Folder watcher active status dot |
| `k.c.info.fg` / `k.c.danger.fg` | `#7698FD` / `#F17471` | Shadow (`BLK %`) and Highlight (`WHT %`) clipping telemetry; `Reset All` destructive action |

---

## 3. Typography & Line-Height Rules

Rune Darkroom uses **Inter** (variable sans) for UI chrome and **IBM Plex Mono** with OpenType tabular numerals (`'tnum' 1`) for every numeric readout, Kelvin value, percentage, Lab coordinate, and keyboard shortcut so dragging a slider never causes horizontal jitter.

### Current vs. Recommended Typography Scale

> **Core Rule:** No body, caption, or popover text may drop below **`1.45×` line-height** (`1.45–1.55×` target range). Eliminate all sub-`10.5px` micro-text and fix the `1.09×` (`11/12`) collision in `section_header` and `caption`.

| Token (`tokens::ty`) | Current (`tokens.rs` / `opencode`) | Current Ratio | Recommended | Rec. Ratio | Usage |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `dialog_title` | `14.0px / 21.0px` (500) | `1.50` | **`15.0px / 22.0px`** (530) | **`1.47`** | Settings/Export card titles, modal headers |
| `brand_title` | `13.0px / 20.0px` (530) | `1.54` | **`13.0px / 19.5px`** (530) | **`1.50`** | Document titlebar / popover titles |
| `dialog_body` | `13.0px / 19.5px` (400) | `1.50` | **`13.0px / 20.0px`** (400) | **`1.54`** | Popover descriptions, modal prose, settings explanations |
| `control_label` | `12.0px / 18.0px` (400) | `1.50` | **`12.5px / 18.5px`** (440) | **`1.48`** | Slider labels, pill text, menu labels, layer names |
| `control_label_medium` | `12.0px / 18.0px` (500) | `1.50` | **`12.5px / 18.5px`** (530) | **`1.48`** | Active tab labels, selected rows, sub-group titles |
| `section_header` | `11.0px / 12.0px` (440, +0.05) | **`1.09` (cramped)** | **`11.5px / 17.0px`** (600, `+0.06em`) | **`1.48`** | Uppercase collapsible card headers (`EXPOSURE & TONE`) |
| `caption` | `11.0px / 12.0px` (440, +0.05) | **`1.09` (cramped)** | **`12.0px / 18.0px`** (400) | **`1.50`** | Popover secondary copy, empty states, status subtitles |
| `mono_value` | `11.0px / 15.0px` (`tnum`) | `1.36` | **`12.0px / 18.0px`** (`tnum`) | **`1.50`** | Scrubbable Capsule numeric readouts (`+0.45 EV`, `5600 K`) |
| `mono_compact` | `10.5px / 14.5px` (`tnum`) | `1.38` | **`11.0px / 16.5px`** (`tnum`) | **`1.50`** | Keycaps (`⌘⇧E`), badge counts, clipping telemetry (`BLK 0.0%`) |
| `mono_status` | `11.0px / 14.0px` (`tnum`) | `1.27` | **`11.5px / 17.0px`** (`tnum`) | **`1.48`** | Bottom status bar EXIF, zoom %, live MCP status |
| `mono_micro` / `mono_probe` | `9.5–10.0px / 13.5–14.0px` | `1.40–1.42` | **`10.5px / 15.5px`** (`tnum`) | **`1.48`** | Live `RGB · HSL · Lab` probe, pinned readouts, CLI key badges |

---

## 4. Spacing & Layout Principles

### 4.1 The 3-Level Darkroom Visual Hierarchy

Every inspector tab must maintain unmistakable spatial separation across three nested tiers:

```text
Level 1 — CARD  (12pt vertical gap between cards · 14pt internal padding · #242424 fill · 8pt radius)
  └── Level 2 — GROUP  (14pt gap between logical groups inside a card · optional 0.5pt hairline or subtle uppercase micro-label)
        └── Level 3 — CONTROL  (8pt gap between stacked capsules · 6pt gap between horizontal pills)
```

### 4.2 Current vs. Recommended Layout & Spacing Tokens

| Token (`tokens.rs`) | Current | Recommended | Range / Notes |
| :--- | :--- | :--- | :--- |
| `size::INSPECTOR_W` | `356pt` | **`388pt`** | `380–396pt` comfortable studio rail; fits long labels + `ⓘ` + mono readouts without truncation |
| `size::CARD_INNER_W` | `308pt` | **`332pt`** | Derived: `388 - 2×14 (outer) - 2×14 (card) = 332pt` |
| Inspector outer padding (`inspector-pinned` & `inspector-tabpanel`) | `10pt` (`space::LG`) | **`14pt`** | Consistent breathing margin from viewport edge and scrollbar |
| Card internal padding (`collapsible_section_card`) | `10pt` (`space::LG`) | **`14pt`** | Eliminates cramped corner-hugging headers and controls |
| Card-to-card vertical gap | `8pt` (`space::MD`) | **`12pt`** | Clear visual break between distinct tool modules |
| Card header-to-body gap | `8pt` (`space::MD`) | **`10pt`** | Clean separation below the `0.5pt` header hairline |
| Intra-card sub-group gap | `5–6pt` (inconsistent) | **`14pt`** | Visually separates presets/modes from sliders or primary from `Advanced ▸` |
| Control-to-control gap (stacked sliders) | `5pt` (`space::TIGHT`) | **`8pt`** | Prevents vertical capsule wall fatigue (`8–10pt` range) |
| Horizontal pill gap | `4pt` (`space::XS`) | **`6pt`** | Clear tap/click separation between radio segments |
| `size::CAPSULE_SLIDER_H` | `30pt` | **`32pt`** | `32–34pt` target; `12pt` internal horizontal padding (was `10pt`) |
| `size::MINI_CAPSULE_SLIDER_H` | `22pt` | **`24pt`** | Color wheel luminance scrubbers |
| `size::PILL_H` | `24pt` | **`28pt`** | Inspector tabs, mode selectors, radio pills (meets macOS HIG comfort) |
| `size::PILL_COMPACT_H` | `22pt` (some `18pt`) | **`26pt`** | Compact action/preset pills; retire `18pt` micro-pills |
| `size::ACTION_PILL_H` | `28pt` | **`30pt`** | Rotate/Flip & primary inline action pills |
| `size::MINI_AUTO_BTN` / header icons | `20pt` | **`22pt`** | `A` Auto, Eyedropper, Reset, and `ⓘ` hit area (`22×22pt` hit target around `14pt` glyph) |
| `size::TITLEBAR_H` | `48pt` | **`50pt`** | `48–52pt` macOS Tahoe unified toolbar height |
| `size::STATUS_BAR_H` | `28pt` | **`30pt`** | Comfortable single-line telemetry bar |
| `size::HISTOGRAM_H` / `PROBE_FOOTER_H` | `64pt` / `16pt` | **`72pt` / `20pt`** | Cleaner RGB/Luma curve separation & taller probe line-height |

---

## 5. Component Stylings

### 5.1 Scrubbable Capsule Slider
- **Geometry:** Full card inner width (`332pt`), `32pt` height, `8pt` corner radius (`k.radius.lg`), `0.5pt` border (`k.c.border_base`), `#161616` track fill, `#2E2E2E` hover fill.
- **Internal Layout:** `12pt` horizontal padding. Left: optional `6pt` channel dot + control label (`12.5px`, `text_muted` at rest, `text_base` when modified) + inline **Tier 2 `ⓘ` badge** (quiet or revealed on row hover). Right: tabular mono readout (`12px`, `'tnum' 1`).
- **Fill Behavior:** Bipolar controls grow strictly from the center `1pt` magnetic zero-detent tick (`capsule_zero_tick`); unipolar controls grow from the left edge. Fill is `rgba(255,255,255,0.13)` with a crisp `1pt` leading edge (`rgba(255,255,255,0.24)`).
- **Interaction:** Horizontal drag (`Cursor::ResizeColumn`) with subtle zero-detent snap (`±1.6%`); double-click resets to registry default; arrow keys nudge `2.5%` (`Shift` = `10%`), `Delete`/`Backspace` resets.

### 5.2 Collapsible Tool Card
- **Surface:** `layer_01` (`#242424`), `8pt` radius, `0.5pt` `border_base` outline, `14pt` internal padding.
- **Header Row (`min-height: 22pt`):**
  - *Left:* `11pt` disclosure chevron (`▾`/`▸`) + `6pt` gap + uppercase title (`11.5px / 17px`, weight `600`, `+0.06em` tracking) + optional `4pt` `#A2BCFF` modified dot + **Tier 2 `ⓘ` info badge** right after the title!
  - *Right:* Compact `22×22pt` header actions (`Eyedropper`, `A` Auto, `Reset`) spaced `4pt` apart.
- **Divider:** When expanded, a `0.5pt` `border_muted` hairline separates header from body with `10pt` breathing room.

### 5.3 Inline Progressive Disclosure Row (`Advanced ▸`)
- **Appearance:** Full-width subtle interactive row (`28pt` height, `6pt` radius, transparent at rest, `overlay-hover` on hover) positioned at the bottom of a card's Tier 0 controls with a `10pt` top margin.
- **Content:** Left: `10pt` chevron (`▸` collapsed / `▾` expanded) + label (e.g., `Fine Range`, `Precision`, `Output Levels`, `Base Mask RGB`, `More Ratios`) in `12px` `text_muted`. Right: compact count badge of how many hidden controls are currently **modified** (e.g., `2 active` in `#A2BCFF` mono pill, or subtle neutral count `3` in `text_faint`).
- **Auto-Visibility Rule:** Never silently hide active edits. If a preset, sidecar, or AI agent modifies a Tier 1 parameter while its drawer is collapsed, display an accented `#A2BCFF` dot + modified count (`Precision · 2 modified ▸`) — and optionally auto-expand on explicit inspect.

### 5.4 Segmented Controls, Radio Pills & Consolidated Dropdowns
- **Height:** `28pt` primary / `26pt` compact (`4pt` radius inside a `6pt` recessed `#080808` container with `2pt` inner padding).
- **Rule of 5:** Never render more than **4–5 pills in a single row** or more than **1 row** of creation buttons in a tool card. When an option set has `6+` choices (e.g., 7 White Balance illuminants, 7 Add-Layer types, 6 Film Grain types, 8 Aspect Ratios), either use a **compact macOS popup button (`As Shot ▾`, `+ Add Layer ▾`)** or keep the top 3–4 at Tier 0 and tuck the remainder in Tier 1.

### 5.5 Scrollbars & Viewport Scroll Hygiene
- Every scrollable container (`inspector-scroll`, `settings-scroll`, `export-scroll`, `grid-scroll`) MUST bind its viewport inside a bounded flex chain (`min_h(0.0)` + `grow(1.0)`), use `k.scroll_bar(...)`, apply a `14pt` bottom scroll fade (`size::SCROLL_FADE`), and consume wheel events cleanly so scrolling the sidebar never zooms the main canvas.

---

## 6. Depth, Elevation & Surfaces

Rune Darkroom relies on **tonal step elevation + `0.5pt` crisp hairlines** rather than heavy drop shadows inside docked panels. Drop shadows are reserved exclusively for **floating layers** above the workspace.

| Layer | Surface | Border | Shadow | Examples |
| :--- | :--- | :--- | :--- | :--- |
| **Z0 — Canvas** | `#121212` | None | None | Photo viewport, Proof Margin mat |
| **Z1 — Chrome Rail** | `#080808` / `#161616` | `1pt` `border_base` edge | None | Titlebar, Inspector Rail, Filmstrip, Status Bar |
| **Z2 — Tool Card** | `#242424` (`layer_01`) | `0.5pt` `border_base` | None | Collapsible darkroom cards |
| **Z3 — Interactive Control** | `#161616` → `#2E2E2E` → `#3A3A3A` | `0.5pt` `border_base` / `1pt` `border_strong` | None | Scrubbable Capsules, active tabs, selected layers |
| **Z4 — Floating Popover / HUD** | `rgba(36, 36, 36, 0.92)` + `14pt` backdrop blur | `0.5pt` `border_strong` | `shadows.popover` | Hover `ⓘ` info popover, Toolbar `Overlays ▾` popover, `+ Add Layer ▾` menu, Viewport HUD |
| **Z5 — Modal / Command Palette** | `#1E1E1E` over `scrim` | `1pt` `border_strong` | `shadows.popover` | `⌘K` Command Palette, About / Close confirmation |

---

## 7. Progressive Disclosure & Hover Info (`ⓘ`) Specification

### 7.1 The Three Disclosure Tiers

1. **Tier 0 — Immediate Studio Controls (Always Visible when Card Expanded):**
   - Strictly the **3–5 highest-frequency controls** photographers adjust 90% of the time.
   - At most **2–3 cards expanded by default** per inspector tab so the user sees the full landscape of a tab without a wall of 25+ sliders.
2. **Tier 1 — Inline `Advanced ▸` / `More…` Disclosure (1 Click Inside Card):**
   - Precision, edge-case, or calibration sliders (e.g., Sharpen Radius/Threshold/Halo, Grain Granularity & 6 Film Types, Film Negative Base R/G/B, Levels Out Shadow/Highlight, Local Adjustment Color/Detail, Keystone Aspect/Amount, Lens Fringing/Falloff).
   - Zero permanent clutter when neutral; immediately displays an accented `#A2BCFF` modified badge whenever a Tier 1 control is non-default.
3. **Tier 2 — Hover `ⓘ` Info Badge & Frosted macOS Popover (Zero Permanent Wall-of-Text):**
   - Replaces every permanent inline instructional sentence in the sidebar (e.g., `"Click to add • Drag • Double-click removes"`, `"Drag corners or edges on photo • Press C to apply"`, `"Click photo to place spot • Drag to aim source patch"`, `"No masks yet — click a shape above..."`) and documents every tool header and setting label on demand!

### 7.2 Hover `ⓘ` Badge & Popover Anatomy

- **Placement:** Immediately trailing every **Tool Card Header** (`EXPOSURE & TONE ⓘ`) and every **Setting / Slider Label** (`Highlights ⓘ`).
- **Icon Geometry:** `14pt × 14pt` circular outline (`1pt` stroke) containing a centered `i` glyph, centered inside a `22pt × 22pt` pointer hit region so hovering is effortless and never fights slider dragging (hovering `ⓘ` pauses capsule resize cursor and switches to `Cursor::Default` / `Pointer`).
- **Visibility Modes (User-configurable in Workbench & Settings):**
  - **`Reveal on Hover` (Recommended Default for Controls):** Invisible (`opacity: 0`) at rest on slider rows; fades in to `opacity: 0.72` (`120ms` ease-out) when hovering the slider row, and `opacity: 1.0` (`text_accent` `#A2BCFF`) when hovering the `ⓘ` itself. Card header `ⓘ` remains subtly visible (`opacity: 0.38`) at rest so discoverability stays high!
  - **`Subtle Always`:** Every `ⓘ` renders at `opacity: 0.35` at rest (`opacity: 1.0` on hover) for users who prefer constant visual signifiers.
  - **`Off`:** Hides `ⓘ` badges for minimalists (shortcuts still appear in menus and `⌘K`).
- **Popover Trigger & Dismissal:** `180ms` hover intent delay (`0ms` on click/keyboard focus) so casual mouse sweeps across sliders never flash popovers; `120ms` grace period when moving pointer onto the popover; opens to the **left** of the inspector (over the dark canvas edge, never clipped by the sidebar scrollbar!) with a `6pt` directional arrow.
- **Popover Content Structure (Max width `272pt`, `12pt` padding, `8pt` radius, frosted `rgba(30,30,30,0.94)` + `14pt` blur):**
  1. **Header Row:** Control / Tool title (`12.5px`, `530`, `#FAFAFA`) + right-aligned **macOS keycap** if bound (e.g., `⌘E`, `B`, `C`, `Double-click to reset`).
  2. **Plain-English Explanation:** 1–2 concise sentences (`12px / 18px`, `1.50` line-height, `#AEAEAE`) explaining *what it actually does to the image* and when to use it.
  3. **Agent / CLI Footer Strip (`0.5pt` top divider, `#080808` inset pill):** Monospace `rd` CLI / MCP parameter key + range (e.g., `rd edit --set highlights=-0.35` · `MCP: highlights [-1..1]`), reinforcing Rune Darkroom's **GUI ↔ CLI ↔ MCP 1:1 parity** without cluttering the sidebar!

### 7.3 Complete Tab-by-Tab Progressive Disclosure Matrix (All 6 Tabs · All 28 Cards)

| Tab | Card | Current Default | Rec. Default | Tier 0 (Primary Controls) | Tier 1 (`Advanced ▸` Disclosure) | Moved to Tier 2 (`ⓘ` Popover) |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Light** | `BASE CHARACTERISTICS` | Open | **Collapsed** (auto-badge if modified) | Mode (`Photo \| Film Neg`) + Film Curve dropdown/4-grid | — | Linear vs. Film curve response & `film_curve` key |
| **Light** | `FILM NEGATIVE` *(cond.)* | Open | **Open** *(when Film Neg active)* | Base swatch, `Pick Base` / `Auto Base`, Density, Contrast, Black Clip, White Clip (6) | **`Base Mask RGB ▸`** Base Red, Base Green, Base Blue (3) | Orange C-41 mask neutralization & `film_negative.*` keys |
| **Light** | `WHITE BALANCE` | Open | **Open** | Preset compact picker (`As Shot ▾`), Temperature, Tint + header Eyedropper / `A` / Reset (3) | **`Illuminant Presets ▸`** (or compact dropdown: Daylight, Cloudy, Shade, Tungsten, Fluorescent, Flash) | Kelvin calibration & `temperature` / `tint` / `wb_preset` |
| **Light** | `EXPOSURE & TONE` | Open | **Open** | Exposure, Contrast, Highlights, Shadows + header `A` / Reset (4) | **`Fine Range ▸`** Brightness, Whites, Blacks (3) | Scene-referred EV math vs. tonal recovery & CLI keys |
| **Light** | `LEVELS` | Closed | **Closed** | Channel tabs (`RGB\|R\|G\|B`), Histogram strip, In Shadow, In Midtone, In Highlight (5) | **`Output Levels ▸`** Out Shadow, Out Highlight (2) | 0.1% Auto Levels clipping & `levels.*` keys |
| **Light** | `TONE CURVE` | Closed | **Closed** | Channel tabs (`RGB\|L\|R\|G\|B`) + Interactive spline graph | — | `"Click to add • Drag • Double-click removes"` + Fritsch-Carlson info |
| **Color** | `PRESENCE` | Open | **Open** | Vibrance, Saturation (2) | — | Skin-protecting Vibrance vs. linear Saturation |
| **Color** | `COLOR EDITOR` | Open | **Open** | Mode (`Basic\|Adv\|Skin`); Basic: 8 swatches + Hue, Sat, Lum | Adv: **`Range & Smoothness ▸`** (Hue/Sat Range, Smoothness, Uniformity); Skin: **`Amount & Smoothness ▸`** (Amount H/S/L, Smoothness) | Eyedropper & `View Selected Color Range` behavior |
| **Color** | `COLOR BALANCE` | Open | **Collapsed** *(or Open if preferred)* | `3-Way \| Master` switcher + Wheels & Luma scrubbers | — | 3-way lift/gamma/gain wheels & `shadows_wheel` etc. |
| **Color** | `BLACK & WHITE` | Closed | **Closed** | Header Enable switch + 6 channel mix sliders | **`Split Tones ▸`** Shadow Hue/Sat, Highlight Hue/Sat (4) | Panchromatic filter emulsion simulation & `bw.*` keys |
| **Detail** | `CLARITY & DEHAZE` | Open | **Open** | Clarity, Structure, Dehaze (3) + active Method badge | **`Method ▸`** Natural / Punch / Neutral / Classic (4) | Local contrast method math & `clarity_method` |
| **Detail** | `SHARPENING` | Open | **Open** | Amount (1) | **`Precision ▸`** Radius, Threshold, Halo Suppression (3) | Unsharp mask halo suppression & `sharpen_*` keys |
| **Detail** | `NOISE REDUCTION` | Open | **Collapsed** | Luminance, Color (2) | — | Edge-preserving chroma vs. luma denoising |
| **Detail** | `FILM GRAIN` | Open | **Collapsed** | Impact (1) + active Type badge | **`Grain Structure ▸`** 6 Grain Types + Granularity (7) | Silver-halide grain profiles & `grain_type` / `grain_size` |
| **Detail** | `VIGNETTING` | Open | **Collapsed** | Amount (1) + active Method badge | **`Method ▸`** Elliptic / Circular / Off-Crop (3) | Crop-aware optical falloff & `vignette_method` |
| **Lens** | `CROP & ASPECT` | Open | **Open** | Interactive Crop Switch + Output px + top 4 ratios (`Free`, `Orig`, `3:2`, `4:5`) | **`More Ratios & Grid ▸`** `1:1`, `4:3`, `16:9`, `2:3` + Composition Grid (`Off\|Thirds\|Golden\|Diag`) | `"Drag corners or edges • Press C to apply"` + `geometry.crop` |
| **Lens** | `ROTATE & FLIP` | Open | **Open** | Straighten slider + `↺ 90°`, `↻ 90°`, `Flip H`, `Flip V` (5) | — | Lossless quarter-turn + continuous straighten |
| **Lens** | `KEYSTONE` | Open | **Collapsed** | Vertical, Horizontal (2) | **`Fine Keystone ▸`** Aspect, Amount (2) | Perspective homography correction & `geometry.keystone_*` |
| **Lens** | `LENS CORRECTION` | Open | **Collapsed** | Distortion, Chromatic Aberration (2) | **`Edge Cleanup ▸`** Purple Fringing, Light Falloff (2) | Radial Brown-Conrady distortion & `lens.*` keys |
| **Lens** | `COMPOSITION GRID` | Open | **Merged into Crop Tier 1** | *(Eliminates standalone 1-control card)* | Lives under `CROP & ASPECT → More Ratios & Grid ▸` | Overlay grid options (`geometry.grid`) |
| **Layers** | `LAYERS` | Open | **Open** | Single `+ Add Layer ▾` menu (replaces 7 pills!), 5-tool strip, Layer stack, Opacity | **`Mask Display & Actions ▸`** Invert, Clear, Display (`Off\|Red\|Gray`) | `M` / `⌥M` overlay shortcuts & `rune_edit_layers` |
| **Layers** | `LOCAL ADJUSTMENTS` | Open | **Open** *(when layer selected)* | Exposure, Contrast, Highlights, Shadows, Temp (5) | **`Color & Detail ▸`** Brightness, Tint, Saturation, Clarity, Dehaze, Sharpen (6) | Per-layer masked blend math & `layers[].adjustments` |
| **Layers** | `COMBINE MASKS` | Open | **Collapsed** *(auto-open on Brush or >1 mask)* | `+ Add Mask ▾` + active mask list (`Add\|Sub\|Int`) + Primary Feather/Range | **`Falloff & Brush ▸`** Luma Low/High Falloff; Brush controls auto-appear when Brush/Eraser active | Boolean mask composition & `layers[].masks` |
| **Layers** | `HEAL & CLONE` | Open | **Collapsed** *(auto-open on `Q`/`S`)* | Mode (`Heal Q \| Clone S`) + Radius, Feather + Spot list | **`Edge & Opacity ▸`** Opacity + Clear Spots | `"Click photo to place spot • Drag to aim source patch • Esc for Hand"` |
| **Style** | `STYLES & PRESETS` | Open | **Open** | 2-column built-in preset cards | — | Non-destructive recipe blend & `--preset` CLI flag |
| **Style** | `USER STYLES` | Open | **Collapsed** | Save Style input + Saved user styles list | **`Blending & Stacking ▸`** Stack Styles toggle + Strength slider | Additive style stacking & `rune_manage_workflow` |
| **Style** | `VIRTUAL VARIANTS` | Open | **Collapsed** *(or compact 1-row strip when >1)* | Active variant pills + `+ New`, `Clone`, `Compare` | — | Multi-look sidecar variants & `⌘⏎` Compare |
| **Style** | `ADJUSTMENTS CLIPBOARD` | Open | **Collapsed** *(auto-open when clipboard filled)* | `Copy`, `Apply Selected`, `Autoselect Adjusted` | **`Selective Groups ▸`** 10-group checkbox matrix | Selective group copy/paste (`⌘⇧C` / `⌘⇧V`); move `Reset All` out of Clipboard into footer/Edit menu |

**Net Visual Calm Impact:** Initial visible controls per tab drop by **55%–84%** (e.g., `Detail`: `25 → 4`, `Light`: `18–27 → 7`, `Lens`: `24 → 10`, `Layers`: `37 → 10`) while preserving **100% of every parameter** within one click!

---

## 8. Menu & Navigation Hierarchy

### 8.1 Native macOS Menu Bar Overhaul (`crates/app/src/menu.rs`) — Locked

**Current Problems Audited in `menu.rs`:**
1. **`View` is a 15-item flat wall** mixing zoom, 5 diagnostic overlays, interactive modes (`Crop`, `Loupe`), workflow settings (`Auto-Advance on Cull`), framing (`Proof Margin`), and chrome panels.
2. **`Edit` mixes text/undo with 5 Variant commands** (`New Variant`, `Clone Variant`, `Next Variant`, `Delete Active Variant`, `Compare Variant`).
3. **Shortcut inconsistencies between `menu.rs` and `toolbar.rs`:**
   - `GPU Loupe Magnifier`: `⌥L` in `menu.rs:321` vs. `O` in `toolbar.rs:161`. *(Locked to `O`, matching Capture One / single-key darkroom ergonomics.)*
   - `Toggle Filmstrip`: `⌘⌥F` in `menu.rs:353` vs. `F` in `toolbar.rs:242`. *(Locked identically across menu & toolbar to `F`.)*
   - `Toggle Inspector`: `⌘⌥I` in `menu.rs:361` vs. `I` in `toolbar.rs:260`. *(Locked identically across menu & toolbar to `I`.)*

**Locked macOS HIG Native Menu Bar (5 primary menus, `≤ 8` visible rows each + `Rune Darkroom` app menu):**

| Top-Level Menu | Prior Count | Locked Visible Rows | Locked Grouping & Submenus |
| :--- | :--- | :--- | :--- |
| **Rune Darkroom** | 2 (+std) | **2 (+std)** | `About Rune Darkroom` · `Settings & Local MCP… (⌘,)` · *(standard Hide/Quit)* |
| **File** | 9 (flat) | **6 + 1 submenu** | `Open Photo… (⌘O)` · `Open Folder… (⌘⇧O)` · **`Session ▸`** (`New Capture One Session…`, `Watch Hot Folder…`, `Move to Selects ⌘J`) — `Save Sidecar (⌘S)` · `Export Recipes… (⌘⇧E)` · `Edit With External Editor… (⌘⇧W)` |
| **Edit** | 10 (mixed) | **5** | `Undo (⌘Z)` · `Redo (⌘⇧Z)` — `Copy Adjustments (⌘⇧C)` · `Paste Selected Adjustments (⌘⇧V)` — `Reset Adjustments (⌘R)` |
| **Image** *(New)* | *(none)* | **4 + 3 submenus** | **`Variants ▸`** (`New Variant`, `Clone Variant`, `Next Variant`, `Delete Active Variant`, `Compare Variant ⌘⏎`) · **`Auto Adjust ▸`** (`Auto White Balance`, `Auto Tone`, `Auto Levels`) · **`Color Tag ▸`** (`None` + 7 Capture One tags) — `Auto-Advance on Cull` |
| **View** | **15 (wall)** | **7 + 3 submenus** | **`Zoom ▸`** (`Fit ⌘0`, `150% ⌘1`, `200% ⌘2`) · **`Overlays & Diagnostics ▸`** (`Exposure Clipping ⌘E`, `Focus Mask ⌘M`, `Red Mask M`, `Grayscale Mask ⌥M`, `GPU Loupe O`, `Crop Frame C`) · **`Proof Margin ▸`** (`Compact`, `Studio`, `Exhibition`, `Cycle ⌘⇧M`) — `Before / After Split (\)` · `Grid Browser (G)` · `Show Filmstrip (F)` · `Show Inspector (I)` |
| **Window** | *(std)* | **≤ 4** | Standard macOS Window commands (`Minimize`, `Zoom`) + `Settings & Local MCP… (⌘,)` |

### 8.2 Top Titlebar Toolbar Overhaul (`crates/app/src/toolbar.rs`) — Locked Concept A

Prior titlebar packed **16 interactive controls (18 click targets) + 5 dividers**. **Concept A is locked for `Studio Airy`:**

- **Concept A — Minimal Pro Titlebar + `Overlays ▾` Popover *(LOCKED — Applied)*:**
  - **Controls:** **8** (`-50%` visual chrome).
  - *Left:* macOS traffic-light clearance (`insets.leading + 14pt`) + Brand icon + Filename + subtle edited pill/dot (move `16-BIT` / detailed telemetry to bottom status bar).
  - *Center:* `Fit | 150% | 200%` segmented zoom + `Compare (\)` + **`Overlays ▾`** popover (houses Exposure Clipping `⌘E`, Focus Mask `⌘M`, Red/Grayscale Mask `M`/`⌥M`, GPU Loupe `O`, Composition Grid, and Proof Margin `⌘⇧M`, displaying a warm-amber active-count indicator when any overlay is active).
  - *Right:* `Grid (G)` + `Settings` (`MCP On` indicator when running, `⌘,`) + **`Export`** primary CTA. (`Open`, `Undo/Redo`, `Filmstrip F`, and `Inspector I` stay 1 keystroke away via macOS Menu Bar + shortcuts.)
- *Explored Workbench Alternatives (non-default):*
  - **Concept B — Centered `⌘K` Command Pill:** Replaces center zoom/compare controls with an instant fuzzy launcher pill.
  - **Concept C — Compact Capture One Left Cursor Rail (`36–42pt`):** Moves canvas cursor tools (`Hand`, `Crop`, `WB Eyedropper`, `Loupe`, `Brush`, `Gradient`, `Heal`) onto a quiet left rail.

---

## 9. Settings View Inheritance Spec (`⌘,` · `crates/app/src/settings_view.rs`)

The full-workspace **Settings** view (`⌘,`) strictly inherits the locked **`Studio Airy`** visual language so transitioning from the darkroom canvas into Settings feels like opening **macOS Tahoe System Settings** calibrated for a monochrome color-grading studio — never a separate crowded dashboard.

### 9.1 Inherited `Studio Airy` Layout & Grouped Card Anatomy

- **Two-Pane macOS System Settings Shell:**
  - **Left Navigation Rail (`232pt` wide, `#080808` `bg_deep`, `1px` right `border_base`):**
    - Top **`← Back to Darkroom`** button (`Esc`) separated by a subtle hairline divider.
    - Uppercase `SETTINGS` header (`11.5px`, `600`, `+0.06em` tracking).
    - 4 clean category items (`Agent & Local MCP`, `CLI & Agent Skill`, `Shortcuts & Workflow`, `Security & Storage`) with `8pt` row gap, `28–32pt` hit height, `#3A3A3A` active pill surface, and live `#47D67A` green status dots (on `Agent & Local MCP` when MCP is running, and `Shortcuts & Workflow` when Hot Folder Watch is active).
    - Footer status caption: `MCP Spec 2025-11-25` · `Localhost 127.0.0.1 only`.
  - **Right Scrollable Content Column (`settings-scroll`, `min_h(0.0)` + `grow(1.0)`, `#121212` `page` backdrop):**
    - Centered `max_w: 740pt` column with `20pt` outer padding and **`12pt` (`CARD_GAP`)** vertical rhythm between grouped cards.
- **macOS System Settings Grouped Cards:**
  - Every card uses the exact locked `Studio Airy` tokens: **`CARD_PAD = 14pt`**, **`CARD_GAP = 12pt`**, **`ROW_GAP = 8pt`**, **`GROUP_GAP = 14pt`**, **`BASE_FONT = 12.5px`**, **`LINE_HEIGHT = 1.50×`**, **`SECTION_TRACKING = +0.06em`**.
  - Surface: `#161616` (`bg_base`) / `#242424` (`layer_01`) with `0.5pt` `border_base` outline and `8–10pt` continuous corners.
  - **Crisp `1px` Row Dividers:** Stacked setting rows inside a grouped card are separated by a clean `1px` hairline (`rgba(255, 255, 255, 0.08)` / `border_muted`) rather than wrapping every toggle in nested boxes.
  - **Single-Line Scannable Rows:** Left side holds concise label (`12.5px`, `530`, `#FAFAFA`) + trailing **`ⓘ` hover badge** + optional compact monospace value/endpoint; right side holds the macOS switch, segmented selector, port presets, or action button.

### 9.2 Moving Dense Helper Prose into Trailing `ⓘ` Hover Popovers (Tier 2)

Permanent 2–3 line explanatory paragraphs below Settings controls create visual fatigue and push key actions below the fold. Under `Studio Airy`, every explanatory description moves into a trailing **`ⓘ` hover popover** (`14pt` circular badge, `Reveal on Hover` with `180ms` intent delay, frosted darkroom popover showing explanation + shortcut/badge + CLI/config path):

| Setting Label | Category | Popover Explanation (Summary) | Shortcut / CLI / Config Footer |
| :--- | :--- | :--- | :--- |
| **`Local MCP Server ⓘ`** | Agent & Local MCP | Runs an embedded MCP (`2025-11-25`) Streamable HTTP server on localhost so AI agents can inspect, analyze, preview, and edit photos non-destructively. | `⌘,` · `rd mcp --http --port 37420` |
| **`Allow Live Canvas Edits ⓘ`** | Agent & Local MCP | Permits connected agents to update the currently open darkroom viewport in real time; every mutation pushes onto your `⌘Z` undo history. | `⌘Z` · `mcp_settings.allow_live_canvas_edits` |
| **`Port Binding ⓘ`** | Agent & Local MCP | Selects the local loopback port (`37420`, `37421`, or `38080`). Strictly bound to `127.0.0.1` — never exposed on Wi-Fi or LAN interfaces. | `127.0.0.1` · `http://127.0.0.1:37420/mcp` |
| **`Agent Snippets ⓘ`** | Agent & Local MCP | Copy-ready connection configs for Streamable HTTP (Antigravity, Claude Code, Cursor, Windsurf), headless Stdio (`rd mcp`), and `SKILL.md`. | `skills/rune-darkroom/SKILL.md` |
| **`Hot Folder Watch ⓘ`** | Shortcuts & Workflow | Monitors a studio capture folder for newly arriving RAW/JPEG files, auto-ingesting each shot and optionally applying a non-destructive Style sidecar. | `rd session watch <dir> --style <name>` |
| **`Proof Margin ⓘ`** | Shortcuts & Workflow | Surrounds the canvas with a calibrated neutral gallery mat (`Compact 16pt`, `Studio 48pt`, or `Exhibition 84pt`) for print and tonal evaluation. | `⌘⇧M` · `settings.proof_margin` |
| **`Custom Shortcuts ⓘ`** | Shortcuts & Workflow | Click any keycap pill to record a new binding (highlighted in C-41 amber `#F28F3D`). Stored cleanly alongside MCP preferences. | `~/.config/rune-darkroom/settings.json` |
| **`Inviolable Originals ⓘ`** | Security & Storage | Originals are never overwritten: `guard_not_source`, RAW extension blocklist, atomic `.tmp` + `sync_all` + `rename` sidecars, and `overwrite: false` export defaults. | `PhotoMeta.revision` · `guard_not_source` |

### 9.3 3-Tier Progressive Disclosure Inside Settings (All 4 Categories)

| Settings Category | Tier 0 — Primary Surface (Always Visible) | Tier 1 — Expandable Inline `▸` Disclosures | Tier 2 — Hover `ⓘ` Popovers |
| :--- | :--- | :--- | :--- |
| **Agent & Local MCP** | Master `Local MCP Server` switch + green status pill; `Allow Live Canvas Edits` switch; `Port Binding` presets (`37420 \| 37421 \| 38080`) + `Copy URL`; activity telemetry strip + `127.0.0.1 + Origin Guard` badge; `Streamable HTTP \| Stdio \| SKILL.md` transport picker + `Copy Snippet` CTA. | **`▸ Connection Config Snippets`** (formatted JSON/CLI config block synced to active port & transport); **`▸ Exposed MCP Tools (9)`** (full 9-tool registry-synced table: `rune_get_schema`, `rune_inspect_photo`, `rune_analyze_photo`, `rune_edit_photo`, `rune_preview_photo`, `rune_diff_photo`, `rune_cull_or_export`, `rune_manage_workflow`, `rune_edit_layers`). | `Local MCP Server ⓘ`, `Allow Live Canvas Edits ⓘ`, `Port Binding ⓘ`, `Agent Snippets ⓘ` |
| **CLI & Agent Skill** | `rd` Binary path + synced parameter count (`74 params`); `SKILL.md` manifest path + Copy button; `GUI ↔ CLI ↔ MCP` 1:1 parity status badge. | **`▸ CLI Workflow Examples (5)`** (`rd schema recipe`, `rd analyze --ndjson`, `rd edit --preset`, `rd diff --assert`, `rd mcp --http`). | `rd Binary Path ⓘ`, `Agent Skill Manifest ⓘ`, `1:1 Parity Contract ⓘ` |
| **Shortcuts & Workflow** | `Hot Folder Watch` status + `Watch Folder…` / `Poll Now` / `Stop`; `Auto-Apply Style on Arrival` pills (`None` + top 4 presets); `Proof Margin` segmented selector (`Compact \| Studio \| Exhibition`); primary rebindable `Custom Shortcuts` list + `Reset Defaults`. | **`▸ Workflow & System Shortcuts (5)`** (`⌘⇧M`, `⌘J`, `⌘⏎`, `⌘⇧E`, `⌘⇧W`). | `Hot Folder Watch ⓘ`, `Auto-Apply Style ⓘ`, `Proof Margin ⓘ`, `Custom Shortcuts ⓘ` |
| **Security & Storage** | Crisp macOS status rows with enforcement badges: `Inviolable Originals` (`Enforced`), `Loopback-Only Binding` (`127.0.0.1 Only`), `Origin & DNS Rebinding Guard` (`RFC 6454 + 403`), and `Settings File` path (`~/.config/rune-darkroom/settings.json`). | **`▸ Security & Sandbox Invariants (6)`** (loopback bind, strict `Host`/`Origin` RFC 6454 + `u16` port check, `/dev/urandom` session IDs without `is_empty()` `DELETE` bypass, bounded body reads/concurrency, atomic sidecar `sync_all` + `rename`, `overwrite: false` export safety). | `Inviolable Originals ⓘ`, `Loopback-Only Binding ⓘ`, `Origin Guard ⓘ`, `Settings Persistence ⓘ` |

---

## 10. Responsive & Window Scaling Behavior

| Window Width | Layout Adaptation |
| :--- | :--- |
| **`≥ 1440pt`** (Studio Display / 16" MBP) | Full `388pt` Inspector + optional `Studio` (`34pt`) or `Exhibition` (`70pt`) Proof Margin + Filmstrip (`160pt`). |
| **`1200–1439pt`** (Default `1360×860`) | `388pt` Inspector + `Compact` Proof Margin; titlebar filename truncates gracefully (`min_w(0.0)` + `shrink()`). |
| **`1024–1199pt`** (Minimum `1024×680`) | Inspector narrows smoothly to `368pt` (`CARD_INNER_W = 312pt`); titlebar collapses `Compare` label to icon-only while preserving `0 overlapping elements` on macOS traffic lights (`insets.leading + 14pt`). |
| **Narrow / Focus Mode** | Press `I` (Inspector) and `F` (Filmstrip) to dedicate 100% of the window to the photograph. |

---

## 11. Do's & Don'ts

### Do
- **Do** keep every chrome surface strictly neutral gray (`#080808`–`#3A3A3A`) so photographic color judgment is never biased.
- **Do** enforce `1.50×` target (`1.45–1.55×` floor) line-height across all readable text and `'tnum' 1` tabular numerals across all live readouts.
- **Do** limit expanded Tier 0 controls to **3–5 per card** and **2–3 open cards per tab** by default.
- **Do** auto-indicate modified Tier 1 controls with a `#A2BCFF` dot or count on the collapsed `Advanced ▸` row so hidden edits are never silent.
- **Do** inherit `Studio Airy` in `Settings` (`⌘,`): macOS System Settings grouped cards (`14pt` pad, `12pt` card gap, `8pt` row gap, `1px` hairline row dividers), trailing `ⓘ` hover popovers, and inline `▸` disclosures for JSON snippets, MCP tool catalogs, and sandbox invariants.
- **Do** place plain-English descriptions, keyboard shortcuts, and `rd` CLI / MCP parameter keys inside **Tier 2 hover `ⓘ` popovers** instead of permanent inline prose.
- **Do** verify every macOS menu shortcut matches its toolbar/popover tooltip verbatim.
- **Do** register every new UI parameter in `crates/core/src/registry.rs` (`SCALAR_PARAMS` + `RecipePatch`) to uphold strict GUI ↔ CLI ↔ MCP 1:1 parity.

### Don't
- **Don't** expand all 4–5 cards on a tab by default (`detail`, `lens`, `layers`, `style` anti-pattern).
- **Don't** dump permanent multi-line helper paragraphs, raw JSON config blocks, or 9-row MCP tool tables onto the Tier 0 Settings surface when a trailing `ⓘ` popover or inline `▸` disclosure keeps Settings calm.
- **Don't** render two stacked rows of 7+ pill buttons when a single macOS popup button (`+ Add Layer ▾`, `As Shot ▾`) suffices.
- **Don't** use `1.09×` line-height (`11px / 12px`), `18pt` micro-pills, or `5pt` control gaps between stacked capsules.
- **Don't** dump 15 flat items into a single macOS menu or 16+ buttons into the titlebar.
- **Don't** introduce decorative gradients, neon chrome accents, or heavy box shadows on docked cards.
- **Don't** open `ⓘ` popovers instantaneously (`0ms`) on hover — always honor the `180ms` hover intent delay so cursor travel stays calm.

---

## 12. Agent Prompt Guide

When updating Rune Darkroom's UI (`crates/app/src/*`), AI coding agents MUST follow this quick-reference contract:

1. **Source of Truth:** Read `DESIGN.md` (`Locked v1 — Studio Airy`) and `crates/app/src/tokens.rs` before adding or resizing any widget. Never hardcode arbitrary pixel values or raw hex colors outside `tokens.rs`.
2. **Spatial Formula:** `INSPECTOR_W = 388pt`, outer padding `14pt`, `CARD_PAD = 14pt`, `CARD_INNER_W = 332pt`, `CARD_GAP = 12pt`, `GROUP_GAP = 14pt`, `ROW_GAP = 8pt`, `CAPSULE_SLIDER_H = 32pt`, pill height `28pt` (compact `26pt`).
3. **Typography Floor:** `BASE_FONT = 12.5px`, `LINE_HEIGHT = 1.50×` (minimum `1.45×`), `SECTION_TRACKING = +0.06em`; minimum `10.5px` mono / `11.5px` section headers / `12px` captions; always `.features("'tnum' 1")` on numbers.
4. **Progressive Disclosure & Settings Inheritance Check:** Before adding any control to an inspector card or Settings pane, classify it as **Tier 0** (top 3–5 primary), **Tier 1** (inline `▸` disclosure drawer with badge), or **Tier 2** (trailing `ⓘ` hover popover with `180ms` intent delay, explanation, shortcut, and `rd`/MCP/config key). Never add permanent instructional helper paragraphs below sliders, graphs, or Settings toggles.
5. **macOS Chrome & Scroll Verification:** Preserve Concept A Minimal Pro Titlebar (`Overlays ▾`), 5 scannable macOS HIG menus (`≤ 8` rows), `insets.leading` traffic-light clearance (`0 overlapping elements`), and bounded flex scroll (`min_h(0.0)` + `grow(1.0)`) on `inspector-scroll`, `settings-scroll`, `export-scroll`, and `grid-scroll`. Verify headless `--chrome --snap` across scenes in `--release` mode.
