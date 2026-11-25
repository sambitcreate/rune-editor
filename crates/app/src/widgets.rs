//! Pro Capture One / Lightroom darkroom inspector widgets styled natively with
//! Anomaly OpenCode (`rune-theme-opencode`) tokens, `rune::Shape` vectors, and `rune::Pixels`.

use rune::{
    Color, Cursor, Drag, El, LineCap, Phase, ScrollSpec, Shape, TextStyle, Tx, col, el, icon, row,
    text,
};
use rune_editor_core::{
    ColorWheel, DevelopRecipe, HISTOGRAM_BINS, HSL_BAND_COUNT, HSL_BAND_NAMES, ToneCurves,
    default_curve_points,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, SegmentedControl,
    Slider,
};
use rune_theme_opencode::icons;

use crate::color_wheel_tex::{
    hsl_band_swatch, pointer_to_wheel_xy, srgb, wheel_pixels, wheel_puck_color, wheel_xy_to_point,
};
use crate::state::{AppState, CurveChannel, InspectorTab};

pub const INSPECTOR_WIDTH: f32 = 336.0;
const CARD_INNER_W: f32 = 288.0;

fn mono_readout(k: &Kit, size: f32) -> TextStyle {
    TextStyle {
        size,
        line_height: size + 4.0,
        ..k.ty.mono
    }
    .features("'tnum' 1")
}

/// Pinned live RGB + Luminance histogram with anti-aliased vector area polygons and
/// shadow/highlight clipping telemetry.
pub fn histogram_panel(s: &AppState, k: &Kit) -> El {
    let w = 292.0_f32;
    let h = 68.0_f32;
    let step = w / (HISTOGRAM_BINS.saturating_sub(1).max(1)) as f32;

    let build_area = |bins: &[f32; HISTOGRAM_BINS]| -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
        let mut line = Vec::with_capacity(HISTOGRAM_BINS);
        let mut poly = Vec::with_capacity(HISTOGRAM_BINS + 2);
        poly.push([0.0, h]);
        for (i, &val) in bins.iter().enumerate() {
            let x = i as f32 * step;
            let y = h - (val.clamp(0.0, 1.0) * (h - 4.0));
            line.push([x, y]);
            poly.push([x, y]);
        }
        poly.push([w, h]);
        (poly, line)
    };

    let (l_poly, l_line) = build_area(&s.histogram.luma);
    let (r_poly, r_line) = build_area(&s.histogram.r);
    let (g_poly, g_line) = build_area(&s.histogram.g);
    let (b_poly, b_line) = build_area(&s.histogram.b);

    let mut shape = Shape::new().view_box(w, h);
    // Subtle quarter tonal-zone grid lines (Blacks / Shadows / Midtones / Highlights / Whites)
    for q in 1..4 {
        let gx = w * (q as f32 * 0.25);
        shape = shape.line([gx, 2.0], [gx, h], 0.5, LineCap::Butt, k.c.border_muted);
    }

    shape = shape
        .fill(l_poly, srgb(0.84, 0.85, 0.88).alpha(0.18))
        .fill(r_poly, srgb(0.95, 0.34, 0.34).alpha(0.28))
        .fill(g_poly, srgb(0.32, 0.86, 0.48).alpha(0.26))
        .fill(b_poly, srgb(0.34, 0.58, 0.98).alpha(0.30))
        .stroke(r_line, 1.0, srgb(0.96, 0.40, 0.40).alpha(0.82))
        .stroke(g_line, 1.0, srgb(0.38, 0.88, 0.52).alpha(0.78))
        .stroke(b_line, 1.0, srgb(0.42, 0.64, 0.98).alpha(0.82))
        .stroke(l_line, 1.0, srgb(0.92, 0.92, 0.94).alpha(0.55));

    let shadow_clip = s.histogram.shadow_clip_pct;
    let highlight_clip = s.histogram.highlight_clip_pct;
    let blk_color = if shadow_clip > 0.5 {
        k.c.info.fg
    } else {
        k.c.text_muted
    };
    let wht_color = if highlight_clip > 0.5 {
        k.c.danger.fg
    } else {
        k.c.text_muted
    };

    let header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            text("HISTOGRAM")
                .font(k.ty.small_medium)
                .color(k.c.text_muted),
        )
        .child(
            row()
                .items_center()
                .gap(8.0)
                .child(
                    text(format!("BLK {shadow_clip:.1}%"))
                        .font(mono_readout(k, 10.5))
                        .color(blk_color),
                )
                .child(
                    text(format!("WHT {highlight_clip:.1}%"))
                        .font(mono_readout(k, 10.5))
                        .color(wht_color),
                ),
        );

    let plot = el()
        .key("histogram-canvas")
        .w_full()
        .h(h)
        .radius(k.radius.sm)
        .shape(shape);

    col()
        .key("histogram-card")
        .w_full()
        .gap(6.0)
        .padding(10.0)
        .radius(k.radius.lg)
        .fill(k.c.bg_deep)
        .border(0.5, k.c.border_base)
        .child(header)
        .child(plot)
}

/// Collapsible OpenCode section card (Capture One accordion behavior).
fn collapsible_section_card(
    k: &Kit,
    key: &'static str,
    title: &'static str,
    open: bool,
    on_toggle: fn(&mut AppState),
    reset_action: Option<El>,
    content: El,
) -> El {
    let chevron = if open {
        &icons::CHEVRON_DOWN
    } else {
        &icons::CHEVRON_RIGHT
    };

    let toggle = row()
        .role("button")
        .label(format!("Toggle {title}"))
        .expanded(open)
        .focusable(true)
        .focus_ring(2.0, 2.0, k.c.border_focus)
        .radius(k.radius.xs)
        .cursor(Cursor::Pointer)
        .items_center()
        .gap(5.0)
        .child(icon(chevron, 11.0).color(k.c.icon_muted).a11y_hidden())
        .child(text(title).font(k.ty.small_medium).color(k.c.text_muted))
        .on_press(on_toggle);

    let mut header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(toggle);
    if let Some(btn) = reset_action {
        header = header.child(btn);
    }

    let mut card = col()
        .key(key)
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(0.5, k.c.border_base)
        .child(header);
    if open {
        card = card.child(content);
    }
    card
}

/// Reusable OpenCode section card with tracked uppercase title and optional reset action.
fn section_card(
    k: &Kit,
    key: &'static str,
    title: &'static str,
    reset_action: Option<El>,
    content: El,
) -> El {
    let mut header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(text(title).font(k.ty.small_medium).color(k.c.text_muted));
    if let Some(btn) = reset_action {
        header = header.child(btn);
    }

    col()
        .key(key)
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(0.5, k.c.border_base)
        .child(header)
        .child(content)
}

fn mini_reset_btn(
    k: &Kit,
    label: &'static str,
    disabled: bool,
    on_press: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    IconButton::new(&icons::OUTLINE_RESET)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Mini)
        .label(label)
        .help(label)
        .disabled(disabled)
        .on_press_tx(on_press)
        .build(k)
}

/// OpenCode-styled photographic slider with click-to-reset label and tabular mono value readout.
#[allow(clippy::too_many_arguments)]
fn develop_slider(
    k: &Kit,
    key: &'static str,
    label: &'static str,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    value_fmt: String,
    reset_val: f32,
    setter: fn(&mut DevelopRecipe, f32),
) -> El {
    let modified = (value - reset_val).abs() > 0.005;
    let header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            el().role("button")
                .label(format!("Reset {label}"))
                .help(format!("Click to reset {label}"))
                .focusable(true)
                .focus_ring(2.0, 2.0, k.c.border_focus)
                .radius(k.radius.xs)
                .cursor(Cursor::Pointer)
                .child(text(label).font(k.ty.text_xsmall).color(k.c.text_base))
                .on_press_tx(move |s: &mut AppState, tx| {
                    s.update_slider_tx(true, tx, |r| setter(r, reset_val));
                }),
        )
        .child(
            text(&value_fmt)
                .font(mono_readout(k, 11.0))
                .color(if modified {
                    k.c.text_accent
                } else {
                    k.c.text_muted
                }),
        );

    let slider = Slider::new(format!("{key}-slider"), value)
        .range(range)
        .label(label)
        .hide_label()
        .value_text(value_fmt)
        .control_size(ControlSize::Mini)
        .width(CARD_INNER_W)
        .on_change_tx(move |s: &mut AppState, v: f32, done: bool, tx| {
            s.update_slider_tx(done, tx, |r| setter(r, v));
        })
        .build(k);

    col().key(key).w_full().gap(2.0).child(header).child(slider)
}

fn channel_color(ch: CurveChannel, k: &Kit) -> Color {
    match ch {
        CurveChannel::Luma => k.c.text_base,
        CurveChannel::Red => srgb(0.95, 0.36, 0.36),
        CurveChannel::Green => srgb(0.35, 0.86, 0.48),
        CurveChannel::Blue => srgb(0.38, 0.62, 0.98),
    }
}

/// Interactive Tone Curve Editor with Fritsch-Carlson monotonic cubic spline preview,
/// Luma / R / G / B channel switcher, background histogram silhouette, and draggable handles.
fn tone_curve_editor(s: &AppState, k: &Kit) -> El {
    let cw = CARD_INNER_W;
    let ch = 144.0_f32;
    let active_ch = s.curve_channel;
    let active_pts = active_ch.points(&s.recipe.curves);
    let active_color = channel_color(active_ch, k);

    // Build background shape: faint luma histogram + 4x4 grid + diagonal + ghost curves + active spline
    let mut shape = Shape::new().view_box(cw, ch);

    // Faint luma histogram silhouette inside curve box
    let step = cw / (HISTOGRAM_BINS.saturating_sub(1).max(1)) as f32;
    let mut hist_poly = Vec::with_capacity(HISTOGRAM_BINS + 2);
    hist_poly.push([0.0, ch]);
    for (i, &val) in s.histogram.luma.iter().enumerate() {
        hist_poly.push([i as f32 * step, ch - val.clamp(0.0, 1.0) * (ch * 0.72)]);
    }
    hist_poly.push([cw, ch]);
    shape = shape.fill(hist_poly, k.c.white.alpha(0.06));

    // 4x4 photographic gridlines
    for i in 1..4 {
        let f = i as f32 * 0.25;
        shape = shape
            .line(
                [cw * f, 0.0],
                [cw * f, ch],
                0.5,
                LineCap::Butt,
                k.c.border_muted,
            )
            .line(
                [0.0, ch * f],
                [cw, ch * f],
                0.5,
                LineCap::Butt,
                k.c.border_muted,
            );
    }

    // Identity diagonal reference
    shape = shape.line([0.0, ch], [cw, 0.0], 0.75, LineCap::Butt, k.c.border_base);

    // Ghost strokes for other modified channels
    for other in CurveChannel::ALL {
        if other == active_ch {
            continue;
        }
        let pts = other.points(&s.recipe.curves);
        if pts.len() == 2 && pts[0] == [0.0, 0.0] && pts[1] == [1.0, 1.0] {
            continue;
        }
        let sampled: Vec<[f32; 2]> = (0..=48)
            .map(|i| {
                let x = i as f32 / 48.0;
                let y = ToneCurves::evaluate_channel(pts, x);
                [x * cw, (1.0 - y) * ch]
            })
            .collect();
        shape = shape.stroke(sampled, 1.25, channel_color(other, k).alpha(0.32));
    }

    // Active channel monotonic cubic spline
    let spline_pts: Vec<[f32; 2]> = (0..=64)
        .map(|i| {
            let x = i as f32 / 64.0;
            let y = ToneCurves::evaluate_channel(active_pts, x);
            [x * cw, (1.0 - y) * ch]
        })
        .collect();
    shape = shape.stroke(spline_pts, 1.75, active_color);

    // Interactive curve canvas with draggable handles
    let mut canvas_box = el()
        .key("tone-curve-canvas")
        .role("group")
        .label(format!("{} tone curve graph", active_ch.label()))
        .w(cw)
        .h(ch)
        .radius(k.radius.md)
        .fill(k.c.bg_deep)
        .border(0.5, k.c.border_base)
        .cursor(Cursor::Crosshair)
        .shape(shape)
        .on_drag(move |s: &mut AppState, d: &mut Drag, tx: &mut Tx| {
            let nx = (d.x / d.w.max(1.0)).clamp(0.0, 1.0);
            let ny = (1.0 - d.y / d.h.max(1.0)).clamp(0.0, 1.0);
            let ch_sel = s.curve_channel;

            match d.phase {
                Phase::Down => {
                    if s.drag_undo_snapshot.is_none() {
                        s.drag_undo_snapshot = Some(s.recipe.clone());
                    }
                    let pts = ch_sel.points_mut(&mut s.recipe.curves);
                    // Find closest existing point within 14pt radius
                    let mut best: Option<(usize, f32)> = None;
                    for (idx, p) in pts.iter().enumerate() {
                        let px = p[0] * d.w;
                        let py = (1.0 - p[1]) * d.h;
                        let dist = (px - d.x).hypot(py - d.y);
                        if dist <= 14.0 && best.is_none_or(|(_, bd)| dist < bd) {
                            best = Some((idx, dist));
                        }
                    }
                    let grabbed = if let Some((idx, _)) = best {
                        idx
                    } else if pts.len() < 12 {
                        let nx_ins = nx.clamp(0.025, 0.975);
                        let insert_at = pts
                            .partition_point(|p| p[0] < nx_ins)
                            .clamp(1, pts.len().saturating_sub(1));
                        pts.insert(insert_at, [nx_ins, ny]);
                        insert_at
                    } else {
                        pts.len() - 1
                    };
                    s.dragging_curve_pt = Some(grabbed);
                    s.recipe.preset_name = None;
                    s.bump_curve_generation();
                    s.recompute_histogram();
                }
                Phase::Move => {
                    if let Some(idx) = s.dragging_curve_pt {
                        let pts = ch_sel.points_mut(&mut s.recipe.curves);
                        if idx < pts.len() {
                            let last = pts.len().saturating_sub(1);
                            let clamped_x = if idx == 0 {
                                0.0
                            } else if idx == last {
                                1.0
                            } else {
                                let min_x = pts[idx - 1][0] + 0.025;
                                let max_x = pts[idx + 1][0] - 0.025;
                                nx.clamp(min_x, max_x.max(min_x))
                            };
                            pts[idx] = [clamped_x, ny];
                            s.recipe.preset_name = None;
                            s.bump_curve_generation();
                            s.catalog.update_active_recipe(s.recipe.clone());
                            s.recompute_histogram();
                            tx.instant();
                        }
                    }
                }
                Phase::Up => {
                    s.dragging_curve_pt = None;
                    if let Some(prev) = s.drag_undo_snapshot.take()
                        && prev != s.recipe
                    {
                        s.history.push(prev);
                    }
                    s.sync_window_state(tx);
                }
            }
        });

    // Control point handles overlaid at exact spline coordinates
    for (idx, &[px, py]) in active_pts.iter().enumerate() {
        let is_dragged = s.dragging_curve_pt == Some(idx);
        let size = if is_dragged { 10.0 } else { 8.0 };
        let cx = (px * cw - size * 0.5).clamp(1.0, cw - size - 1.0);
        let cy = ((1.0 - py) * ch - size * 0.5).clamp(1.0, ch - size - 1.0);
        canvas_box = canvas_box.child(
            el().key(format!("curve-pt-{idx}"))
                .absolute(cx, cy)
                .size(size, size)
                .radius(size * 0.5)
                .fill(if is_dragged {
                    active_color
                } else {
                    k.c.bg_base
                })
                .border(1.5, active_color)
                .a11y_hidden(),
        );
    }

    // Channel pill switcher row
    let mut ch_row = row()
        .role("radiogroup")
        .label("Tone curve channel")
        .items_center()
        .gap(4.0);
    for ch_opt in CurveChannel::ALL {
        let active = ch_opt == active_ch;
        let dot_col = channel_color(ch_opt, k);
        let pill = row()
            .key(format!("curve-ch-{}", ch_opt.short_label()))
            .role("radio")
            .label(format!("{} curve channel", ch_opt.label()))
            .checked(active)
            .focusable(true)
            .focus_ring(2.0, 1.0, k.c.border_focus)
            .cursor(Cursor::Pointer)
            .items_center()
            .gap(5.0)
            .h(24.0)
            .px(8.0)
            .radius(k.radius.sm)
            .fill(if active { k.c.layer_03 } else { k.c.bg_base })
            .hover(k.c.hover)
            .border(
                0.5,
                if active {
                    k.c.border_strong
                } else {
                    k.c.border_muted
                },
            )
            .child(el().size(6.0, 6.0).radius(3.0).fill(dot_col).a11y_hidden())
            .child(
                text(ch_opt.short_label())
                    .font(k.ty.small_medium)
                    .color(if active {
                        k.c.text_base
                    } else {
                        k.c.text_muted
                    }),
            )
            .on_press(move |s: &mut AppState| {
                s.curve_channel = ch_opt;
                s.dragging_curve_pt = None;
            });
        ch_row = ch_row.child(pill);
    }

    let channel_is_flat = active_pts == default_curve_points().as_slice();
    let controls_row = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(ch_row)
        .child(
            text(format!("{} pts", active_pts.len()))
                .font(mono_readout(k, 10.5))
                .color(k.c.text_faint),
        );

    let body = col()
        .w_full()
        .gap(8.0)
        .child(controls_row)
        .child(canvas_box)
        .child(
            text("Click curve to add point  •  Drag handles to shape")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );

    let reset = mini_reset_btn(k, "Reset active curve channel", channel_is_flat, |s, tx| {
        s.reset_active_curve_channel_tx(tx);
    });

    collapsible_section_card(
        k,
        "sec-tone-curve",
        "TONE CURVE",
        s.curve_open,
        |s| s.curve_open = !s.curve_open,
        Some(reset),
        body,
    )
}

#[derive(Clone, Copy)]
enum WheelSlot {
    Shadows,
    Midtones,
    Highlights,
}

impl WheelSlot {
    fn title(self) -> &'static str {
        match self {
            Self::Shadows => "Shadows",
            Self::Midtones => "Midtones",
            Self::Highlights => "Highlights",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Shadows => "wheel-shadows",
            Self::Midtones => "wheel-midtones",
            Self::Highlights => "wheel-highlights",
        }
    }

    fn get(self, r: &DevelopRecipe) -> ColorWheel {
        match self {
            Self::Shadows => r.shadows_wheel,
            Self::Midtones => r.midtones_wheel,
            Self::Highlights => r.highlights_wheel,
        }
    }

    fn get_mut(self, r: &mut DevelopRecipe) -> &mut ColorWheel {
        match self {
            Self::Shadows => &mut r.shadows_wheel,
            Self::Midtones => &mut r.midtones_wheel,
            Self::Highlights => &mut r.highlights_wheel,
        }
    }
}

fn single_color_wheel(s: &AppState, k: &Kit, slot: WheelSlot) -> El {
    let diameter = 88.0_f32;
    let wheel = slot.get(&s.recipe);
    let puck_pt = wheel_xy_to_point(wheel.hue_x, wheel.hue_y, diameter);
    let center = [diameter * 0.5, diameter * 0.5];
    let modified = !wheel.is_neutral();

    let mut overlay = Shape::new()
        .view_box(diameter, diameter)
        // Subtle center crosshair
        .line(
            [center[0] - 5.0, center[1]],
            [center[0] + 5.0, center[1]],
            0.75,
            LineCap::Round,
            k.c.white.alpha(0.28),
        )
        .line(
            [center[0], center[1] - 5.0],
            [center[0], center[1] + 5.0],
            0.75,
            LineCap::Round,
            k.c.white.alpha(0.28),
        );

    // Tether line from center to puck when offset
    if wheel.hue_x.hypot(wheel.hue_y) > 0.02 {
        overlay = overlay.line(center, puck_pt, 1.25, LineCap::Round, k.c.white.alpha(0.65));
    }

    let puck_size = 11.0_f32;
    let puck = el()
        .key(format!("{}-puck", slot.key()))
        .absolute(puck_pt[0] - puck_size * 0.5, puck_pt[1] - puck_size * 0.5)
        .size(puck_size, puck_size)
        .radius(puck_size * 0.5)
        .fill(wheel_puck_color(wheel.hue_x, wheel.hue_y))
        .border(1.5, k.c.white)
        .shadow(1.0, 3.0, k.c.black.alpha(0.6))
        .a11y_hidden();

    let disc = el()
        .key(format!("{}-disc", slot.key()))
        .role("slider")
        .label(format!("{} color balance wheel", slot.title()))
        .help(format!("Drag to tint {} hue and chroma", slot.title()))
        .focusable(true)
        .focus_ring(2.0, 2.0, k.c.border_focus)
        .size(diameter, diameter)
        .radius(diameter * 0.5)
        .pixels(wheel_pixels())
        .border(
            1.0,
            if modified {
                k.c.border_strong
            } else {
                k.c.border_base
            },
        )
        .shape(overlay)
        .cursor(Cursor::Crosshair)
        .child(puck)
        .on_drag(move |s: &mut AppState, d: &mut Drag, tx: &mut Tx| {
            let (hx, hy) = pointer_to_wheel_xy(d.x, d.y, d.w.max(1.0));
            let done = matches!(d.phase, Phase::Up);
            if matches!(d.phase, Phase::Move) {
                tx.instant();
            }
            s.update_slider_tx(done, tx, |r| {
                let w = slot.get_mut(r);
                w.hue_x = hx;
                w.hue_y = hy;
            });
        });

    let title_btn = row()
        .role("button")
        .label(format!("Reset {} wheel", slot.title()))
        .help(format!("Click to reset {} color wheel", slot.title()))
        .focusable(true)
        .focus_ring(2.0, 1.0, k.c.border_focus)
        .cursor(Cursor::Pointer)
        .items_center()
        .gap(4.0)
        .child(
            text(slot.title())
                .font(k.ty.small_medium)
                .color(if modified {
                    k.c.text_base
                } else {
                    k.c.text_muted
                }),
        )
        .on_press_tx(move |s: &mut AppState, tx| {
            s.update_slider_tx(true, tx, |r| {
                *slot.get_mut(r) = ColorWheel::default();
            });
        });

    let luma_slider = Slider::new(format!("{}-luma", slot.key()), wheel.luma)
        .range(-1.0..=1.0)
        .label(format!("{} luminance", slot.title()))
        .hide_label()
        .value_text(format!("{:+.2}", wheel.luma))
        .control_size(ControlSize::Mini)
        .width(diameter)
        .on_change_tx(move |s: &mut AppState, v: f32, done: bool, tx| {
            s.update_slider_tx(done, tx, |r| {
                slot.get_mut(r).luma = v;
            });
        })
        .build(k);

    let luma_readout = text(format!("Y {:+.2}", wheel.luma))
        .font(mono_readout(k, 10.0))
        .color(if wheel.luma.abs() > 0.005 {
            k.c.text_accent
        } else {
            k.c.text_faint
        });

    col()
        .key(slot.key())
        .w(diameter)
        .items_center()
        .gap(6.0)
        .child(title_btn)
        .child(disc)
        .child(luma_slider)
        .child(luma_readout)
}

/// Capture One-style 3-Way Color Balance Wheels (Shadows / Midtones / Highlights).
fn color_wheels_section(s: &AppState, k: &Kit) -> El {
    let all_neutral = s.recipe.shadows_wheel.is_neutral()
        && s.recipe.midtones_wheel.is_neutral()
        && s.recipe.highlights_wheel.is_neutral();

    let wheels_row = row()
        .w_full()
        .justify_between()
        .items_start()
        .child(single_color_wheel(s, k, WheelSlot::Shadows))
        .child(single_color_wheel(s, k, WheelSlot::Midtones))
        .child(single_color_wheel(s, k, WheelSlot::Highlights));

    let reset = mini_reset_btn(k, "Reset all color wheels", all_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |r| {
            r.shadows_wheel = ColorWheel::default();
            r.midtones_wheel = ColorWheel::default();
            r.highlights_wheel = ColorWheel::default();
        });
    });

    section_card(
        k,
        "sec-color-wheels",
        "COLOR BALANCE",
        Some(reset),
        wheels_row,
    )
}

/// Capture One-style 8-Band Color Editor (HSL) with circular hue swatches.
fn hsl_editor_section(s: &AppState, k: &Kit) -> El {
    let active_idx = s.selected_hsl_band.min(HSL_BAND_COUNT - 1);
    let band = s.recipe.hsl[active_idx];
    let band_name = HSL_BAND_NAMES[active_idx];

    let mut swatch_row = row()
        .key("hsl-swatches")
        .role("radiogroup")
        .label("Color band")
        .w_full()
        .justify_between()
        .items_center();

    for i in 0..HSL_BAND_COUNT {
        let selected = i == active_idx;
        let modified = !s.recipe.hsl[i].is_neutral();
        let swatch_col = hsl_band_swatch(i);

        let btn = col()
            .key(format!("hsl-swatch-{i}"))
            .role("radio")
            .label(HSL_BAND_NAMES[i])
            .checked(selected)
            .help(format!(
                "Edit {} hue, saturation and luminance",
                HSL_BAND_NAMES[i]
            ))
            .focusable(true)
            .focus_ring(2.0, 1.0, k.c.border_focus)
            .cursor(Cursor::Pointer)
            .items_center()
            .gap(3.0)
            .child(
                row()
                    .size(26.0, 26.0)
                    .center()
                    .radius(13.0)
                    .fill(if selected {
                        k.c.layer_03
                    } else {
                        Color::TRANSPARENT
                    })
                    .hover(k.c.hover)
                    .border(
                        if selected { 1.5 } else { 0.5 },
                        if selected {
                            k.c.border_focus
                        } else {
                            Color::TRANSPARENT
                        },
                    )
                    .child(
                        el().size(14.0, 14.0)
                            .radius(7.0)
                            .fill(swatch_col)
                            .border(0.5, k.c.white.alpha(0.25))
                            .a11y_hidden(),
                    ),
            )
            .child(
                el().size(4.0, 4.0)
                    .radius(2.0)
                    .fill(if modified {
                        k.c.text_accent
                    } else {
                        Color::TRANSPARENT
                    })
                    .a11y_hidden(),
            )
            .on_press(move |s: &mut AppState| {
                s.selected_hsl_band = i;
            });

        swatch_row = swatch_row.child(btn);
    }

    let band_header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            row()
                .items_center()
                .gap(6.0)
                .child(
                    el().size(8.0, 8.0)
                        .radius(4.0)
                        .fill(hsl_band_swatch(active_idx))
                        .a11y_hidden(),
                )
                .child(
                    text(format!("{band_name} Band"))
                        .font(k.ty.text_xsmall_medium)
                        .color(k.c.text_base),
                ),
        )
        .child(mini_reset_btn(
            k,
            "Reset color band",
            band.is_neutral(),
            move |s, tx| {
                s.update_slider_tx(true, tx, |r| {
                    r.hsl[active_idx] = rune_editor_core::HslBand::default();
                });
            },
        ));

    let hue_slider = hsl_band_slider(
        k,
        "sl-hsl-hue",
        "Hue",
        band.hue,
        format!("{:+.0}°", band.hue * 30.0),
        active_idx,
        |b, v| b.hue = v,
    );
    let sat_slider = hsl_band_slider(
        k,
        "sl-hsl-sat",
        "Saturation",
        band.saturation,
        format!("{:+.0}", band.saturation * 100.0),
        active_idx,
        |b, v| b.saturation = v,
    );
    let lum_slider = hsl_band_slider(
        k,
        "sl-hsl-lum",
        "Luminance",
        band.luminance,
        format!("{:+.0}", band.luminance * 100.0),
        active_idx,
        |b, v| b.luminance = v,
    );

    let all_hsl_neutral = s.recipe.hsl.iter().all(|b| b.is_neutral());
    let reset_all = mini_reset_btn(k, "Reset all HSL bands", all_hsl_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |r| {
            r.hsl = [rune_editor_core::HslBand::default(); HSL_BAND_COUNT];
        });
    });

    let body = col()
        .w_full()
        .gap(7.0)
        .child(swatch_row)
        .child(band_header)
        .child(hue_slider)
        .child(sat_slider)
        .child(lum_slider);

    section_card(k, "sec-hsl", "COLOR EDITOR (HSL)", Some(reset_all), body)
}

fn hsl_band_slider(
    k: &Kit,
    key: &'static str,
    label: &'static str,
    value: f32,
    value_fmt: String,
    band_idx: usize,
    setter: fn(&mut rune_editor_core::HslBand, f32),
) -> El {
    let modified = value.abs() > 0.005;
    let header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            el().role("button")
                .label(format!("Reset {label}"))
                .focusable(true)
                .focus_ring(2.0, 2.0, k.c.border_focus)
                .cursor(Cursor::Pointer)
                .child(text(label).font(k.ty.text_xsmall).color(k.c.text_base))
                .on_press_tx(move |s: &mut AppState, tx| {
                    s.update_slider_tx(true, tx, |r| setter(&mut r.hsl[band_idx], 0.0));
                }),
        )
        .child(
            text(&value_fmt)
                .font(mono_readout(k, 11.0))
                .color(if modified {
                    k.c.text_accent
                } else {
                    k.c.text_muted
                }),
        );

    let slider = Slider::new(format!("{key}-{band_idx}"), value)
        .range(-1.0..=1.0)
        .label(label)
        .hide_label()
        .value_text(value_fmt)
        .control_size(ControlSize::Mini)
        .width(CARD_INNER_W)
        .on_change_tx(move |s: &mut AppState, v: f32, done: bool, tx| {
            s.update_slider_tx(done, tx, |r| setter(&mut r.hsl[band_idx], v));
        })
        .build(k);

    col().key(key).w_full().gap(2.0).child(header).child(slider)
}

fn light_tab_content(s: &AppState, k: &Kit) -> El {
    let r = &s.recipe;

    let wb_neutral = r.temperature.abs() < 0.005 && r.tint.abs() < 0.005;
    let wb_controls = col()
        .w_full()
        .gap(6.0)
        .child(develop_slider(
            k,
            "sl-temp",
            "Temperature",
            r.temperature,
            -1.0..=1.0,
            format!("{:+.0}", r.temperature * 100.0),
            0.0,
            |rec, v| rec.temperature = v,
        ))
        .child(develop_slider(
            k,
            "sl-tint",
            "Tint",
            r.tint,
            -1.0..=1.0,
            format!("{:+.0}", r.tint * 100.0),
            0.0,
            |rec, v| rec.tint = v,
        ));
    let wb_reset = mini_reset_btn(k, "Reset White Balance", wb_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.temperature = 0.0;
            rec.tint = 0.0;
        });
    });

    let tone_neutral = r.exposure.abs() < 0.005
        && r.contrast.abs() < 0.005
        && r.highlights.abs() < 0.005
        && r.shadows.abs() < 0.005
        && r.whites.abs() < 0.005
        && r.blacks.abs() < 0.005;
    let tone_controls = col()
        .w_full()
        .gap(5.0)
        .child(develop_slider(
            k,
            "sl-exposure",
            "Exposure",
            r.exposure,
            -4.0..=4.0,
            format!("{:+.2} EV", r.exposure),
            0.0,
            |rec, v| rec.exposure = v,
        ))
        .child(develop_slider(
            k,
            "sl-contrast",
            "Contrast",
            r.contrast,
            -1.0..=1.0,
            format!("{:+.0}", r.contrast * 100.0),
            0.0,
            |rec, v| rec.contrast = v,
        ))
        .child(develop_slider(
            k,
            "sl-highlights",
            "Highlights",
            r.highlights,
            -1.0..=1.0,
            format!("{:+.0}", r.highlights * 100.0),
            0.0,
            |rec, v| rec.highlights = v,
        ))
        .child(develop_slider(
            k,
            "sl-shadows",
            "Shadows",
            r.shadows,
            -1.0..=1.0,
            format!("{:+.0}", r.shadows * 100.0),
            0.0,
            |rec, v| rec.shadows = v,
        ))
        .child(develop_slider(
            k,
            "sl-whites",
            "Whites",
            r.whites,
            -1.0..=1.0,
            format!("{:+.0}", r.whites * 100.0),
            0.0,
            |rec, v| rec.whites = v,
        ))
        .child(develop_slider(
            k,
            "sl-blacks",
            "Blacks",
            r.blacks,
            -1.0..=1.0,
            format!("{:+.0}", r.blacks * 100.0),
            0.0,
            |rec, v| rec.blacks = v,
        ));
    let tone_reset = mini_reset_btn(k, "Reset Light & Tone", tone_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.exposure = 0.0;
            rec.contrast = 0.0;
            rec.highlights = 0.0;
            rec.shadows = 0.0;
            rec.whites = 0.0;
            rec.blacks = 0.0;
        });
    });

    col()
        .w_full()
        .gap(8.0)
        .child(collapsible_section_card(
            k,
            "sec-wb",
            "WHITE BALANCE",
            s.wb_open,
            |s| s.wb_open = !s.wb_open,
            Some(wb_reset),
            wb_controls,
        ))
        .child(collapsible_section_card(
            k,
            "sec-tone",
            "EXPOSURE & TONE",
            s.tone_open,
            |s| s.tone_open = !s.tone_open,
            Some(tone_reset),
            tone_controls,
        ))
        .child(tone_curve_editor(s, k))
}

fn color_tab_content(s: &AppState, k: &Kit) -> El {
    let r = &s.recipe;
    let presence_neutral = r.vibrance.abs() < 0.005 && r.saturation.abs() < 0.005;
    let presence_controls = col()
        .w_full()
        .gap(7.0)
        .child(develop_slider(
            k,
            "sl-vibrance",
            "Vibrance",
            r.vibrance,
            -1.0..=1.0,
            format!("{:+.0}", r.vibrance * 100.0),
            0.0,
            |rec, v| rec.vibrance = v,
        ))
        .child(develop_slider(
            k,
            "sl-saturation",
            "Saturation",
            r.saturation,
            -1.0..=1.0,
            format!("{:+.0}", r.saturation * 100.0),
            0.0,
            |rec, v| rec.saturation = v,
        ));
    let presence_reset = mini_reset_btn(
        k,
        "Reset Vibrance & Saturation",
        presence_neutral,
        |s, tx| {
            s.update_slider_tx(true, tx, |rec| {
                rec.vibrance = 0.0;
                rec.saturation = 0.0;
            });
        },
    );

    col()
        .w_full()
        .gap(10.0)
        .child(color_wheels_section(s, k))
        .child(section_card(
            k,
            "sec-presence",
            "PRESENCE",
            Some(presence_reset),
            presence_controls,
        ))
        .child(hsl_editor_section(s, k))
}

fn detail_tab_content(s: &AppState, k: &Kit) -> El {
    let r = &s.recipe;
    let detail_neutral =
        r.clarity.abs() < 0.005 && r.sharpen.abs() < 0.005 && r.noise_reduction.abs() < 0.005;
    let detail_controls = col()
        .w_full()
        .gap(10.0)
        .child(develop_slider(
            k,
            "sl-clarity",
            "Clarity",
            r.clarity,
            -1.0..=1.0,
            format!("{:+.0}", r.clarity * 100.0),
            0.0,
            |rec, v| rec.clarity = v,
        ))
        .child(develop_slider(
            k,
            "sl-sharpen",
            "Sharpening",
            r.sharpen,
            0.0..=1.0,
            format!("{:.0}", r.sharpen * 100.0),
            0.0,
            |rec, v| rec.sharpen = v,
        ))
        .child(develop_slider(
            k,
            "sl-denoise",
            "Noise Reduction",
            r.noise_reduction,
            0.0..=1.0,
            format!("{:.0}", r.noise_reduction * 100.0),
            0.0,
            |rec, v| rec.noise_reduction = v,
        ));
    let detail_reset = mini_reset_btn(k, "Reset Detail", detail_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.clarity = 0.0;
            rec.sharpen = 0.0;
            rec.noise_reduction = 0.0;
        });
    });

    let fx_neutral = r.vignette.abs() < 0.005 && r.grain.abs() < 0.005;
    let fx_controls = col()
        .w_full()
        .gap(10.0)
        .child(develop_slider(
            k,
            "sl-vignette",
            "Vignette",
            r.vignette,
            -1.0..=1.0,
            format!("{:+.0}", r.vignette * 100.0),
            0.0,
            |rec, v| rec.vignette = v,
        ))
        .child(develop_slider(
            k,
            "sl-grain",
            "Film Grain",
            r.grain,
            0.0..=1.0,
            format!("{:.0}", r.grain * 100.0),
            0.0,
            |rec, v| rec.grain = v,
        ));
    let fx_reset = mini_reset_btn(k, "Reset Optics & Film", fx_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |rec| {
            rec.vignette = 0.0;
            rec.grain = 0.0;
        });
    });

    col()
        .w_full()
        .gap(10.0)
        .child(section_card(
            k,
            "sec-detail",
            "DETAIL & SHARPENING",
            Some(detail_reset),
            detail_controls,
        ))
        .child(section_card(
            k,
            "sec-optics",
            "OPTICS & FILM",
            Some(fx_reset),
            fx_controls,
        ))
}

fn presets_tab_content(s: &AppState, k: &Kit) -> El {
    let mut grid_rows = col().w_full().gap(6.0);
    for chunk in s.presets.chunks(2) {
        let mut r = row().w_full().gap(6.0);
        for preset in chunk {
            let active = s.recipe.preset_name.as_deref() == Some(preset.name);
            let id = preset.id;
            let name = preset.name;
            let subtitle = if preset.recipe.saturation <= -0.95 {
                "Monochrome"
            } else if preset.recipe.temperature > 0.1 {
                "Warm Analog"
            } else if preset.recipe.clarity > 0.18 {
                "Crisp Studio"
            } else if preset.recipe.is_neutral() {
                "Linear Reset"
            } else {
                "Editorial Color"
            };

            let card = col()
                .key(format!("preset-{id}"))
                .role("button")
                .label(format!("Apply style {name}"))
                .focusable(true)
                .focus_ring(2.0, 1.0, k.c.border_focus)
                .cursor(Cursor::Pointer)
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .padding(10.0)
                .radius(k.radius.md)
                .fill(if active { k.c.layer_03 } else { k.c.bg_base })
                .hover(k.c.hover)
                .border(
                    if active { 1.0 } else { 0.5 },
                    if active {
                        k.c.border_focus
                    } else {
                        k.c.border_base
                    },
                )
                .child(text(name).font(k.ty.text_xsmall_medium).color(if active {
                    k.c.text_accent
                } else {
                    k.c.text_base
                }))
                .child(text(subtitle).font(k.ty.small).color(k.c.text_faint))
                .on_press_tx(move |s: &mut AppState, tx| s.apply_preset_tx(id, tx));

            r = r.child(card);
        }
        grid_rows = grid_rows.child(r);
    }

    let clipboard_actions = row()
        .w_full()
        .gap(8.0)
        .child(
            Button::new("Copy Look")
                .variant(ButtonVariant::Neutral)
                .control_size(ControlSize::Regular)
                .icon(&icons::COPY)
                .shortcut("cmd+shift+c")
                .on_press_tx(|s: &mut AppState, tx| s.copy_adjustments_tx(tx))
                .build(k),
        )
        .child(
            Button::new("Paste Look")
                .variant(ButtonVariant::Neutral)
                .control_size(ControlSize::Regular)
                .shortcut("cmd+shift+v")
                .disabled(!s.catalog.has_clipboard())
                .on_press_tx(|s: &mut AppState, tx| s.paste_adjustments_tx(tx))
                .build(k),
        );

    let reset_all_row = row().w_full().gap(8.0).child(
        Button::new("Reset All Adjustments")
            .variant(ButtonVariant::Danger)
            .control_size(ControlSize::Regular)
            .icon(&icons::OUTLINE_RESET)
            .shortcut("cmd+r")
            .disabled(s.recipe.is_neutral())
            .on_press_tx(|s: &mut AppState, tx| s.reset_recipe_tx(tx))
            .build(k),
    );

    col()
        .w_full()
        .gap(10.0)
        .child(section_card(
            k,
            "sec-styles",
            "STYLES & PRESETS",
            None,
            grid_rows,
        ))
        .child(section_card(
            k,
            "sec-clipboard",
            "BATCH ADJUSTMENTS",
            None,
            col()
                .w_full()
                .gap(8.0)
                .child(clipboard_actions)
                .child(reset_all_row),
        ))
}

/// Complete tabbed Capture One / OpenCode right inspector with pinned Histogram and
/// native OpenCode scroll bar.
pub fn right_inspector(s: &AppState, k: &Kit) -> El {
    let tabs = SegmentedControl::new(&InspectorTab::LABELS, Some(s.active_tab.index()))
        .label("Inspector tabs")
        .as_tabs()
        .width(None)
        .controls(0, "inspector-tabpanel")
        .controls(1, "inspector-tabpanel")
        .controls(2, "inspector-tabpanel")
        .controls(3, "inspector-tabpanel")
        .on_pick(|s: &mut AppState, i, _| {
            s.active_tab = InspectorTab::from_index(i);
        })
        .build(k);

    let pinned_header = col()
        .key("inspector-pinned")
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_base)
        .child(histogram_panel(s, k))
        .child(tabs);

    let tab_body = match s.active_tab {
        InspectorTab::Light => light_tab_content(s, k),
        InspectorTab::Color => color_tab_content(s, k),
        InspectorTab::Detail => detail_tab_content(s, k),
        InspectorTab::Presets => presets_tab_content(s, k),
    };

    let scroll_content = col()
        .key("inspector-tabpanel")
        .role("tabpanel")
        .label(format!(
            "{} adjustments",
            InspectorTab::LABELS[s.active_tab.index()]
        ))
        .w_full()
        .padding(10.0)
        .child(tab_body);

    let scroll_viewport = el()
        .key("inspector-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .clip_fade(0.0, 0.0, 0.0, 14.0)
        .child(scroll_content);

    col()
        .key("develop-inspector")
        .w(INSPECTOR_WIDTH)
        .h_full()
        .min_h(0.0)
        .fill(k.c.bg_base)
        .border_sides([0.0, 0.0, 0.0, 1.0], k.c.border_base)
        .child(pinned_header)
        .child(k.scroll_bar(scroll_viewport))
}
