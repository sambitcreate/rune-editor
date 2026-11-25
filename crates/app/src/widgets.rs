//! Pro Capture One / Lightroom darkroom inspector widgets styled exclusively with
//! centralized Anomaly OpenCode tokens (`crate::tokens`).

use rune::{
    Color, Cursor, Drag, El, LineCap, Phase, ScrollSpec, Shape, Tx, col, el, icon, row, text,
};
use rune_editor_core::{
    BlackAndWhiteParams, ColorWheel, CompositionGrid, CropAspect, DevelopRecipe, HISTOGRAM_BINS,
    HSL_BAND_COUNT, HSL_BAND_NAMES, LevelsParams, ToneCurves, default_curve_points,
};
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, SegmentedControl,
    Slider, Switch,
};
use rune_theme_opencode::icons;

use crate::color_wheel_tex::{
    pointer_to_wheel_xy, wheel_pixels, wheel_puck_color, wheel_xy_to_point,
};
use crate::state::{AppState, CurveChannel, InspectorTab};
use crate::tokens::{self, border, color, radius, size, space, ty};

/// Pinned live RGB + Luminance histogram with shadow/highlight clipping telemetry
/// and a fixed-height live cursor `RGB • HSL • Lab` color readout footer.
pub fn histogram_panel(s: &AppState, k: &Kit) -> El {
    let w = size::HISTOGRAM_W;
    let h = size::HISTOGRAM_H;
    let step = w / (HISTOGRAM_BINS.saturating_sub(1).max(1)) as f32;

    let build_area = |bins: &[f32; HISTOGRAM_BINS]| -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
        let mut line = Vec::with_capacity(HISTOGRAM_BINS);
        let mut poly = Vec::with_capacity(HISTOGRAM_BINS + 2);
        poly.push([0.0, h]);
        for (i, &val) in bins.iter().enumerate() {
            let x = i as f32 * step;
            let y = h - (val.clamp(0.0, 1.0) * (h - space::XS));
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
    for q in 1..4 {
        let gx = w * (q as f32 * 0.25);
        shape = shape.line(
            [gx, space::XXS],
            [gx, h],
            border::HAIRLINE,
            LineCap::Butt,
            k.c.border_muted,
        );
    }

    shape = shape
        .fill(l_poly, color::hist_luma_fill())
        .fill(r_poly, color::hist_r_fill())
        .fill(g_poly, color::hist_g_fill())
        .fill(b_poly, color::hist_b_fill())
        .stroke(r_line, border::REGULAR, color::hist_r_stroke())
        .stroke(g_line, border::REGULAR, color::hist_g_stroke())
        .stroke(b_line, border::REGULAR, color::hist_b_stroke())
        .stroke(l_line, border::REGULAR, color::hist_luma_stroke());

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
                .font(ty::section_header(k))
                .color(k.c.text_muted),
        )
        .child(
            row()
                .items_center()
                .gap(space::MD)
                .child(
                    text(format!("BLK {shadow_clip:.1}%"))
                        .font(ty::mono_compact(k))
                        .color(blk_color),
                )
                .child(
                    text(format!("WHT {highlight_clip:.1}%"))
                        .font(ty::mono_compact(k))
                        .color(wht_color),
                ),
        );

    let plot = el()
        .key("histogram-canvas")
        .w_full()
        .h(h)
        .radius(k.radius.sm)
        .shape(shape);

    // Fixed-height 1-line color probe footer so hovering never causes layout jitter.
    let probe_footer = if let Some(c) = s.cursor_readout {
        let swatch = el()
            .size(size::PROBE_SWATCH, size::PROBE_SWATCH)
            .radius(radius::PROBE_SWATCH)
            .fill(tokens::srgb(
                c.rgb8[0] as f32 / 255.0,
                c.rgb8[1] as f32 / 255.0,
                c.rgb8[2] as f32 / 255.0,
            ))
            .border(border::HAIRLINE, k.c.border_strong)
            .a11y_hidden();
        row()
            .key("color-probe-footer")
            .w_full()
            .h(size::PROBE_FOOTER_H)
            .items_center()
            .justify_between()
            .gap(space::XS)
            .child(
                row().items_center().gap(space::XS).child(swatch).child(
                    text(format!(
                        "RGB {:>3} {:>3} {:>3}",
                        c.rgb8[0], c.rgb8[1], c.rgb8[2]
                    ))
                    .font(ty::mono_probe(k))
                    .color(k.c.text_base),
                ),
            )
            .child(
                text(format!(
                    "HSL {:.0}° {:.0} {:.0} · Lab {:.0} {:+.0} {:+.0}",
                    c.hsl[0], c.hsl[1], c.hsl[2], c.lab[0], c.lab[1], c.lab[2]
                ))
                .font(ty::mono_probe(k))
                .color(k.c.text_muted),
            )
    } else {
        row()
            .key("color-probe-footer")
            .w_full()
            .h(size::PROBE_FOOTER_H)
            .items_center()
            .justify_between()
            .child(
                text("RGB · HSL · Lab")
                    .font(ty::mono_micro(k))
                    .color(k.c.text_faint),
            )
            .child(
                text("Hover photo to sample")
                    .font(ty::caption(k))
                    .color(k.c.text_faint),
            )
    };

    col()
        .key("histogram-card")
        .w_full()
        .gap(space::TIGHT)
        .padding(space::LG)
        .radius(k.radius.lg)
        .fill(k.c.bg_deep)
        .border(border::HAIRLINE, k.c.border_base)
        .child(header)
        .child(plot)
        .child(probe_footer)
}

/// Collapsible OpenCode section card (Capture One accordion behavior).
fn collapsible_section_card(
    k: &Kit,
    key: &'static str,
    title: &'static str,
    open: bool,
    on_toggle: fn(&mut AppState),
    header_actions: Option<El>,
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
        .focus_ring(border::FOCUS_RING, border::FOCUS_OFFSET, k.c.border_focus)
        .radius(k.radius.xs)
        .cursor(Cursor::Pointer)
        .items_center()
        .gap(space::TIGHT)
        .child(
            icon(chevron, size::ICON_SM)
                .color(k.c.icon_muted)
                .a11y_hidden(),
        )
        .child(
            text(title)
                .font(ty::section_header(k))
                .color(k.c.text_muted),
        )
        .on_press(on_toggle);

    let mut header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(toggle);
    if let Some(actions) = header_actions {
        header = header.child(actions);
    }

    let mut card = col()
        .key(key)
        .w_full()
        .gap(space::MD)
        .padding(space::LG)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(border::HAIRLINE, k.c.border_base)
        .child(header);
    if open {
        card = card.child(content);
    }
    card
}

/// Reusable OpenCode section card with tracked uppercase title and optional header actions.
fn section_card(
    k: &Kit,
    key: &'static str,
    title: &'static str,
    header_actions: Option<El>,
    content: El,
) -> El {
    let mut header = row().w_full().justify_between().items_center().child(
        text(title)
            .font(ty::section_header(k))
            .color(k.c.text_muted),
    );
    if let Some(actions) = header_actions {
        header = header.child(actions);
    }

    col()
        .key(key)
        .w_full()
        .gap(space::MD)
        .padding(space::LG)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(border::HAIRLINE, k.c.border_base)
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

/// Capture One-style compact `A` (Auto) pill button for tool card headers.
fn mini_auto_btn(
    k: &Kit,
    key: &'static str,
    help: &'static str,
    on_press: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    row()
        .key(key)
        .role("button")
        .label(help)
        .help(help)
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .size(size::MINI_AUTO_BTN, size::MINI_AUTO_BTN)
        .center()
        .radius(k.radius.sm)
        .fill(k.c.bg_base)
        .hover(k.c.hover)
        .border(border::HAIRLINE, k.c.border_base)
        .child(text("A").font(ty::mono_compact(k)).color(k.c.text_muted))
        .on_press_tx(on_press)
}

/// Unified OpenCode photographic slider with click-to-reset label and tabular mono readout.
#[allow(clippy::too_many_arguments)]
fn develop_slider_inner(
    k: &Kit,
    key: impl Into<String>,
    label: &'static str,
    dot_color: Option<Color>,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    value_fmt: String,
    reset_val: f32,
    refresh_histogram: bool,
    setter: impl Fn(&mut DevelopRecipe, f32) + Copy + 'static,
) -> El {
    let key_str = key.into();
    let modified = (value - reset_val).abs() > 0.005;

    let mut label_row = row().items_center().gap(space::TIGHT);
    if let Some(col_dot) = dot_color {
        label_row = label_row.child(
            el().size(size::CHANNEL_DOT, size::CHANNEL_DOT)
                .radius(radius::circle(size::CHANNEL_DOT))
                .fill(col_dot)
                .a11y_hidden(),
        );
    }
    label_row = label_row.child(text(label).font(ty::control_label(k)).color(k.c.text_base));

    let header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            el().role("button")
                .label(format!("Reset {label}"))
                .help(format!("Click to reset {label}"))
                .focusable(true)
                .focus_ring(border::FOCUS_RING, border::FOCUS_OFFSET, k.c.border_focus)
                .radius(k.radius.xs)
                .cursor(Cursor::Pointer)
                .child(label_row)
                .on_press_tx(move |s: &mut AppState, tx| {
                    if refresh_histogram {
                        s.update_slider_tx(true, tx, |r| setter(r, reset_val));
                    } else {
                        s.update_geometry_slider_tx(true, tx, |r| setter(r, reset_val));
                    }
                }),
        )
        .child(text(&value_fmt).font(ty::mono_value(k)).color(if modified {
            k.c.text_accent
        } else {
            k.c.text_muted
        }));

    let slider = Slider::new(format!("{key_str}-slider"), value)
        .range(range)
        .label(label)
        .hide_label()
        .value_text(value_fmt)
        .control_size(ControlSize::Mini)
        .width(size::CARD_INNER_W)
        .on_change_tx(move |s: &mut AppState, v: f32, done: bool, tx| {
            if refresh_histogram {
                s.update_slider_tx(done, tx, |r| setter(r, v));
            } else {
                s.update_geometry_slider_tx(done, tx, |r| setter(r, v));
            }
        })
        .build(k);

    col()
        .key(key_str)
        .w_full()
        .gap(space::XXS)
        .child(header)
        .child(slider)
}

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
    develop_slider_inner(
        k, key, label, None, value, range, value_fmt, reset_val, true, setter,
    )
}

fn channel_color(ch: CurveChannel, k: &Kit) -> Color {
    match ch {
        CurveChannel::Luma => k.c.text_base,
        CurveChannel::Red => color::channel_red(),
        CurveChannel::Green => color::channel_green(),
        CurveChannel::Blue => color::channel_blue(),
    }
}

/// Interactive Levels card with mini luma histogram + shadow/midtone/highlight ticks,
/// Auto Levels (`A`) action, and Input/Output point sliders.
fn levels_section(s: &AppState, k: &Kit) -> El {
    let lv = s.recipe.levels.sanitized();
    let w = size::CARD_INNER_W;
    let h = size::LEVELS_STRIP_H;
    let step = w / (HISTOGRAM_BINS.saturating_sub(1).max(1)) as f32;

    let mut poly = Vec::with_capacity(HISTOGRAM_BINS + 2);
    poly.push([0.0, h]);
    for (i, &val) in s.histogram.luma.iter().enumerate() {
        poly.push([i as f32 * step, h - val.clamp(0.0, 1.0) * (h - space::XS)]);
    }
    poly.push([w, h]);

    let bx = lv.in_black * w;
    let wx = lv.in_white * w;
    let mx = bx + (wx - bx) * (0.5 - lv.in_mid * 0.35).clamp(0.05, 0.95);

    let shape = Shape::new()
        .view_box(w, h)
        .fill(poly, color::levels_hist_fill(k))
        .line(
            [bx, 0.0],
            [bx, h],
            border::ACTIVE,
            LineCap::Butt,
            k.c.info.fg,
        )
        .line(
            [mx, space::XXS],
            [mx, h],
            border::REGULAR,
            LineCap::Butt,
            k.c.text_muted,
        )
        .line(
            [wx, 0.0],
            [wx, h],
            border::ACTIVE,
            LineCap::Butt,
            k.c.text_accent,
        );

    let strip = el()
        .key("levels-strip")
        .w(w)
        .h(h)
        .radius(k.radius.sm)
        .fill(k.c.bg_deep)
        .border(border::HAIRLINE, k.c.border_base)
        .shape(shape);

    let controls = col()
        .w_full()
        .gap(space::TIGHT)
        .child(strip)
        .child(develop_slider(
            k,
            "sl-lvl-in-black",
            "In Shadow",
            lv.in_black,
            0.0..=0.95,
            format!("{:.0}", lv.in_black * 255.0),
            0.0,
            |rec, v| {
                rec.levels.in_black = v.min(rec.levels.in_white - 0.02).clamp(0.0, 0.95);
            },
        ))
        .child(develop_slider(
            k,
            "sl-lvl-in-mid",
            "In Midtone",
            lv.in_mid,
            -1.0..=1.0,
            format!("{:+.0}", lv.in_mid * 100.0),
            0.0,
            |rec, v| rec.levels.in_mid = v,
        ))
        .child(develop_slider(
            k,
            "sl-lvl-in-white",
            "In Highlight",
            lv.in_white,
            0.05..=1.0,
            format!("{:.0}", lv.in_white * 255.0),
            1.0,
            |rec, v| {
                rec.levels.in_white = v.max(rec.levels.in_black + 0.02).clamp(0.05, 1.0);
            },
        ))
        .child(develop_slider(
            k,
            "sl-lvl-out-black",
            "Out Shadow",
            lv.out_black,
            0.0..=1.0,
            format!("{:.0}", lv.out_black * 255.0),
            0.0,
            |rec, v| rec.levels.out_black = v,
        ))
        .child(develop_slider(
            k,
            "sl-lvl-out-white",
            "Out Highlight",
            lv.out_white,
            0.0..=1.0,
            format!("{:.0}", lv.out_white * 255.0),
            1.0,
            |rec, v| rec.levels.out_white = v,
        ));

    let actions = row()
        .items_center()
        .gap(space::XS)
        .child(mini_auto_btn(
            k,
            "btn-auto-levels",
            "Auto Levels (0.1% clip)",
            |s, tx| s.auto_levels_tx(tx),
        ))
        .child(mini_reset_btn(
            k,
            "Reset Levels",
            lv.is_identity(),
            |s, tx| {
                s.update_slider_tx(true, tx, |rec| rec.levels = LevelsParams::default());
            },
        ));

    collapsible_section_card(
        k,
        "sec-levels",
        "LEVELS",
        s.levels_open,
        |s| s.levels_open = !s.levels_open,
        Some(actions),
        controls,
    )
}

/// Interactive Tone Curve Editor with Fritsch-Carlson monotonic cubic spline preview,
/// Luma / R / G / B channel switcher, background histogram silhouette, and draggable handles.
fn tone_curve_editor(s: &AppState, k: &Kit) -> El {
    let cw = size::CARD_INNER_W;
    let ch = size::CURVE_H;
    let active_ch = s.curve_channel;
    let active_pts = active_ch.points(&s.recipe.curves);
    let active_color = channel_color(active_ch, k);

    let mut shape = Shape::new().view_box(cw, ch);

    let step = cw / (HISTOGRAM_BINS.saturating_sub(1).max(1)) as f32;
    let mut hist_poly = Vec::with_capacity(HISTOGRAM_BINS + 2);
    hist_poly.push([0.0, ch]);
    for (i, &val) in s.histogram.luma.iter().enumerate() {
        hist_poly.push([i as f32 * step, ch - val.clamp(0.0, 1.0) * (ch * 0.72)]);
    }
    hist_poly.push([cw, ch]);
    shape = shape.fill(hist_poly, color::curve_bg_hist(k));

    for i in 1..4 {
        let f = i as f32 * 0.25;
        shape = shape
            .line(
                [cw * f, 0.0],
                [cw * f, ch],
                border::HAIRLINE,
                LineCap::Butt,
                k.c.border_muted,
            )
            .line(
                [0.0, ch * f],
                [cw, ch * f],
                border::HAIRLINE,
                LineCap::Butt,
                k.c.border_muted,
            );
    }

    shape = shape.line(
        [0.0, ch],
        [cw, 0.0],
        border::THIN,
        LineCap::Butt,
        k.c.border_base,
    );

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
        shape = shape.stroke(
            sampled,
            border::CURVE_GHOST,
            color::curve_ghost(channel_color(other, k)),
        );
    }

    let spline_pts: Vec<[f32; 2]> = (0..=64)
        .map(|i| {
            let x = i as f32 / 64.0;
            let y = ToneCurves::evaluate_channel(active_pts, x);
            [x * cw, (1.0 - y) * ch]
        })
        .collect();
    shape = shape.stroke(spline_pts, border::CURVE_ACTIVE, active_color);

    let mut canvas_box = el()
        .key("tone-curve-canvas")
        .role("group")
        .label(format!("{} tone curve graph", active_ch.label()))
        .w(cw)
        .h(ch)
        .radius(k.radius.md)
        .fill(k.c.bg_deep)
        .border(border::HAIRLINE, k.c.border_base)
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
                            s.recompute_histogram();
                            tx.instant();
                        }
                    }
                }
                Phase::Up => {
                    s.dragging_curve_pt = None;
                    s.catalog.update_active_recipe(s.recipe.clone());
                    if let Some(prev) = s.drag_undo_snapshot.take()
                        && prev != s.recipe
                    {
                        s.history.push(prev);
                    }
                    s.sync_window_state(tx);
                }
            }
        });

    for (idx, &[px, py]) in active_pts.iter().enumerate() {
        let is_dragged = s.dragging_curve_pt == Some(idx);
        let pt_sz = if is_dragged {
            size::CURVE_PT_DRAG
        } else {
            size::CURVE_PT
        };
        let cx = (px * cw - pt_sz * 0.5).clamp(space::MICRO, cw - pt_sz - space::MICRO);
        let cy = ((1.0 - py) * ch - pt_sz * 0.5).clamp(space::MICRO, ch - pt_sz - space::MICRO);
        canvas_box = canvas_box.child(
            el().key(format!("curve-pt-{idx}"))
                .absolute(cx, cy)
                .size(pt_sz, pt_sz)
                .radius(radius::circle(pt_sz))
                .fill(if is_dragged {
                    active_color
                } else {
                    k.c.bg_base
                })
                .border(border::ACTIVE, active_color)
                .a11y_hidden(),
        );
    }

    let mut ch_row = row()
        .role("radiogroup")
        .label("Tone curve channel")
        .items_center()
        .gap(space::XS);
    for ch_opt in CurveChannel::ALL {
        let active = ch_opt == active_ch;
        let dot_col = channel_color(ch_opt, k);
        let pill = row()
            .key(format!("curve-ch-{}", ch_opt.short_label()))
            .role("radio")
            .label(format!("{} curve channel", ch_opt.label()))
            .checked(active)
            .focusable(true)
            .focus_ring(
                border::FOCUS_RING,
                border::FOCUS_OFFSET_TIGHT,
                k.c.border_focus,
            )
            .cursor(Cursor::Pointer)
            .items_center()
            .gap(space::TIGHT)
            .h(size::PILL_H)
            .px(space::MD)
            .radius(k.radius.sm)
            .fill(color::pill_fill(k, active))
            .hover(k.c.hover)
            .border(
                border::HAIRLINE,
                if active {
                    k.c.border_strong
                } else {
                    k.c.border_muted
                },
            )
            .child(
                el().size(size::CHANNEL_DOT, size::CHANNEL_DOT)
                    .radius(radius::circle(size::CHANNEL_DOT))
                    .fill(dot_col)
                    .a11y_hidden(),
            )
            .child(
                text(ch_opt.short_label())
                    .font(ty::section_header(k))
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
                .font(ty::mono_compact(k))
                .color(k.c.text_faint),
        );

    let body = col()
        .w_full()
        .gap(space::MD)
        .child(controls_row)
        .child(canvas_box)
        .child(
            text("Click curve to add point  •  Drag handles to shape")
                .font(ty::caption(k))
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
    let diameter = size::WHEEL_DIAMETER;
    let wheel = slot.get(&s.recipe);
    let puck_pt = wheel_xy_to_point(wheel.hue_x, wheel.hue_y, diameter);
    let center = [diameter * 0.5, diameter * 0.5];
    let arm = size::WHEEL_CROSSHAIR_ARM;
    let modified = !wheel.is_neutral();

    let mut overlay = Shape::new()
        .view_box(diameter, diameter)
        .line(
            [center[0] - arm, center[1]],
            [center[0] + arm, center[1]],
            border::THIN,
            LineCap::Round,
            color::wheel_crosshair(k),
        )
        .line(
            [center[0], center[1] - arm],
            [center[0], center[1] + arm],
            border::THIN,
            LineCap::Round,
            color::wheel_crosshair(k),
        );

    if wheel.hue_x.hypot(wheel.hue_y) > 0.02 {
        overlay = overlay.line(
            center,
            puck_pt,
            border::CURVE_GHOST,
            LineCap::Round,
            color::wheel_tether(k),
        );
    }

    let puck_size = size::WHEEL_PUCK;
    let puck = el()
        .key(format!("{}-puck", slot.key()))
        .absolute(puck_pt[0] - puck_size * 0.5, puck_pt[1] - puck_size * 0.5)
        .size(puck_size, puck_size)
        .radius(radius::circle(puck_size))
        .fill(wheel_puck_color(wheel.hue_x, wheel.hue_y))
        .border(border::ACTIVE, k.c.white)
        .shadow(
            border::PUCK_SHADOW_Y,
            border::PUCK_SHADOW_BLUR,
            color::puck_shadow(k),
        )
        .a11y_hidden();

    let disc = el()
        .key(format!("{}-disc", slot.key()))
        .role("slider")
        .label(format!("{} color balance wheel", slot.title()))
        .help(format!("Drag to tint {} hue and chroma", slot.title()))
        .focusable(true)
        .focus_ring(border::FOCUS_RING, border::FOCUS_OFFSET, k.c.border_focus)
        .size(diameter, diameter)
        .radius(radius::circle(diameter))
        .pixels(wheel_pixels())
        .border(
            border::REGULAR,
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
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .items_center()
        .gap(space::XS)
        .child(
            text(slot.title())
                .font(ty::section_header(k))
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
        .font(ty::mono_micro(k))
        .color(if wheel.luma.abs() > 0.005 {
            k.c.text_accent
        } else {
            k.c.text_faint
        });

    col()
        .key(slot.key())
        .w(diameter)
        .items_center()
        .gap(space::SM)
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

/// Collapsible 6-Channel Black & White Color Sensitivity Mixer.
fn bw_section(s: &AppState, k: &Kit) -> El {
    type BwChannelSpec = (
        &'static str,
        &'static str,
        Color,
        f32,
        fn(&mut DevelopRecipe, f32),
    );
    let bw = s.recipe.bw;
    let channels: [BwChannelSpec; 6] = [
        ("sl-bw-red", "Red", color::channel_red(), bw.red, |r, v| {
            r.bw.red = v;
        }),
        (
            "sl-bw-yellow",
            "Yellow",
            color::channel_yellow(),
            bw.yellow,
            |r, v| r.bw.yellow = v,
        ),
        (
            "sl-bw-green",
            "Green",
            color::channel_green(),
            bw.green,
            |r, v| r.bw.green = v,
        ),
        (
            "sl-bw-cyan",
            "Cyan",
            color::channel_cyan(),
            bw.cyan,
            |r, v| r.bw.cyan = v,
        ),
        (
            "sl-bw-blue",
            "Blue",
            color::channel_blue(),
            bw.blue,
            |r, v| r.bw.blue = v,
        ),
        (
            "sl-bw-magenta",
            "Magenta",
            color::channel_magenta(),
            bw.magenta,
            |r, v| r.bw.magenta = v,
        ),
    ];

    let mut body = col().w_full().gap(space::TIGHT);
    for (key, label, dot, val, setter) in channels {
        body = body.child(develop_slider_inner(
            k,
            key,
            label,
            Some(dot),
            val,
            -1.0..=1.0,
            format!("{:+.0}", val * 100.0),
            0.0,
            true,
            setter,
        ));
    }

    let enable_sw = Switch::new(bw.enabled)
        .label("Enable Black & White")
        .hide_label()
        .on_toggle_tx(|s: &mut AppState, tx| {
            let next_on = !s.recipe.bw.enabled;
            if next_on {
                s.bw_open = true;
            }
            s.update_slider_tx(true, tx, |rec| rec.bw.enabled = next_on);
        })
        .build(k);

    let reset = mini_reset_btn(k, "Reset Black & White", bw.is_neutral(), |s, tx| {
        s.update_slider_tx(true, tx, |rec| rec.bw = BlackAndWhiteParams::default());
    });

    let actions = row()
        .items_center()
        .gap(space::SM)
        .child(enable_sw)
        .child(reset);

    collapsible_section_card(
        k,
        "sec-bw",
        "BLACK & WHITE",
        s.bw_open,
        |s| s.bw_open = !s.bw_open,
        Some(actions),
        body,
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
        let swatch_col = color::hsl_band_swatch(i);

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
            .focus_ring(
                border::FOCUS_RING,
                border::FOCUS_OFFSET_TIGHT,
                k.c.border_focus,
            )
            .cursor(Cursor::Pointer)
            .items_center()
            .gap(space::TINY)
            .child(
                row()
                    .size(size::SWATCH_RING, size::SWATCH_RING)
                    .center()
                    .radius(radius::circle(size::SWATCH_RING))
                    .fill(if selected {
                        k.c.layer_03
                    } else {
                        Color::TRANSPARENT
                    })
                    .hover(k.c.hover)
                    .border(
                        if selected {
                            border::ACTIVE
                        } else {
                            border::HAIRLINE
                        },
                        if selected {
                            k.c.border_focus
                        } else {
                            Color::TRANSPARENT
                        },
                    )
                    .child(
                        el().size(size::SWATCH_DOT, size::SWATCH_DOT)
                            .radius(radius::circle(size::SWATCH_DOT))
                            .fill(swatch_col)
                            .border(border::HAIRLINE, color::swatch_rim(k))
                            .a11y_hidden(),
                    ),
            )
            .child(
                el().size(size::MODIFIED_DOT, size::MODIFIED_DOT)
                    .radius(radius::circle(size::MODIFIED_DOT))
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
                .gap(space::SM)
                .child(
                    el().size(size::BAND_DOT, size::BAND_DOT)
                        .radius(radius::circle(size::BAND_DOT))
                        .fill(color::hsl_band_swatch(active_idx))
                        .a11y_hidden(),
                )
                .child(
                    text(format!("{band_name} Band"))
                        .font(ty::control_label_medium(k))
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

    let hue_slider = develop_slider_inner(
        k,
        format!("sl-hsl-hue-{active_idx}"),
        "Hue",
        None,
        band.hue,
        -1.0..=1.0,
        format!("{:+.0}°", band.hue * 30.0),
        0.0,
        true,
        move |r, v| r.hsl[active_idx].hue = v,
    );
    let sat_slider = develop_slider_inner(
        k,
        format!("sl-hsl-sat-{active_idx}"),
        "Saturation",
        None,
        band.saturation,
        -1.0..=1.0,
        format!("{:+.0}", band.saturation * 100.0),
        0.0,
        true,
        move |r, v| r.hsl[active_idx].saturation = v,
    );
    let lum_slider = develop_slider_inner(
        k,
        format!("sl-hsl-lum-{active_idx}"),
        "Luminance",
        None,
        band.luminance,
        -1.0..=1.0,
        format!("{:+.0}", band.luminance * 100.0),
        0.0,
        true,
        move |r, v| r.hsl[active_idx].luminance = v,
    );

    let all_hsl_neutral = s.recipe.hsl.iter().all(|b| b.is_neutral());
    let reset_all = mini_reset_btn(k, "Reset all HSL bands", all_hsl_neutral, |s, tx| {
        s.update_slider_tx(true, tx, |r| {
            r.hsl = [rune_editor_core::HslBand::default(); HSL_BAND_COUNT];
        });
    });

    let body = col()
        .w_full()
        .gap(space::CONTROL)
        .child(swatch_row)
        .child(band_header)
        .child(hue_slider)
        .child(sat_slider)
        .child(lum_slider);

    section_card(k, "sec-hsl", "COLOR EDITOR (HSL)", Some(reset_all), body)
}

fn light_tab_content(s: &AppState, k: &Kit) -> El {
    let r = &s.recipe;

    let wb_neutral = r.temperature.abs() < 0.005 && r.tint.abs() < 0.005;
    let wb_controls = col()
        .w_full()
        .gap(space::SM)
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
    let wb_picker = IconButton::new(&icons::SELECT_ELEMENT)
        .variant(if s.wb_eyedropper_active {
            IconButtonVariant::Contrast
        } else {
            IconButtonVariant::GhostMuted
        })
        .control_size(ControlSize::Mini)
        .label("Pick White Balance Eyedropper")
        .help("Click any neutral gray pixel on the photo to neutralize white balance")
        .on_press_tx(|s: &mut AppState, tx| s.toggle_wb_eyedropper_tx(tx))
        .build(k);
    let wb_actions = row()
        .items_center()
        .gap(space::XS)
        .child(wb_picker)
        .child(mini_auto_btn(
            k,
            "btn-auto-wb",
            "Auto White Balance",
            |s, tx| s.auto_white_balance_tx(tx),
        ))
        .child(mini_reset_btn(
            k,
            "Reset White Balance",
            wb_neutral,
            |s, tx| {
                s.update_slider_tx(true, tx, |rec| {
                    rec.temperature = 0.0;
                    rec.tint = 0.0;
                });
            },
        ));

    let tone_neutral = r.exposure.abs() < 0.005
        && r.contrast.abs() < 0.005
        && r.brightness.abs() < 0.005
        && r.highlights.abs() < 0.005
        && r.shadows.abs() < 0.005
        && r.whites.abs() < 0.005
        && r.blacks.abs() < 0.005;
    let tone_controls = col()
        .w_full()
        .gap(space::TIGHT)
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
            "sl-brightness",
            "Brightness",
            r.brightness,
            -1.0..=1.0,
            format!("{:+.0}", r.brightness * 100.0),
            0.0,
            |rec, v| rec.brightness = v,
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
    let tone_actions = row()
        .items_center()
        .gap(space::XS)
        .child(mini_auto_btn(
            k,
            "btn-auto-tone",
            "Auto Exposure & Tone",
            |s, tx| s.auto_tone_tx(tx),
        ))
        .child(mini_reset_btn(
            k,
            "Reset Exposure & Tone",
            tone_neutral,
            |s, tx| {
                s.update_slider_tx(true, tx, |rec| {
                    rec.exposure = 0.0;
                    rec.contrast = 0.0;
                    rec.brightness = 0.0;
                    rec.highlights = 0.0;
                    rec.shadows = 0.0;
                    rec.whites = 0.0;
                    rec.blacks = 0.0;
                });
            },
        ));

    col()
        .w_full()
        .gap(space::MD)
        .child(collapsible_section_card(
            k,
            "sec-wb",
            "WHITE BALANCE",
            s.wb_open,
            |s| s.wb_open = !s.wb_open,
            Some(wb_actions),
            wb_controls,
        ))
        .child(collapsible_section_card(
            k,
            "sec-tone",
            "EXPOSURE & TONE",
            s.tone_open,
            |s| s.tone_open = !s.tone_open,
            Some(tone_actions),
            tone_controls,
        ))
        .child(levels_section(s, k))
        .child(tone_curve_editor(s, k))
}

fn color_tab_content(s: &AppState, k: &Kit) -> El {
    let r = &s.recipe;
    let presence_neutral = r.vibrance.abs() < 0.005 && r.saturation.abs() < 0.005;
    let presence_controls = col()
        .w_full()
        .gap(space::CONTROL)
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
        .gap(space::LG)
        .child(color_wheels_section(s, k))
        .child(section_card(
            k,
            "sec-presence",
            "PRESENCE",
            Some(presence_reset),
            presence_controls,
        ))
        .child(bw_section(s, k))
        .child(hsl_editor_section(s, k))
}

fn detail_tab_content(s: &AppState, k: &Kit) -> El {
    let r = &s.recipe;
    let detail_neutral = r.clarity.abs() < 0.005
        && r.structure.abs() < 0.005
        && r.dehaze.abs() < 0.005
        && r.sharpen.abs() < 0.005
        && r.noise_reduction.abs() < 0.005;
    let detail_controls = col()
        .w_full()
        .gap(space::MD)
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
            "sl-structure",
            "Structure",
            r.structure,
            -1.0..=1.0,
            format!("{:+.0}", r.structure * 100.0),
            0.0,
            |rec, v| rec.structure = v,
        ))
        .child(develop_slider(
            k,
            "sl-dehaze",
            "Dehaze",
            r.dehaze,
            -1.0..=1.0,
            format!("{:+.0}", r.dehaze * 100.0),
            0.0,
            |rec, v| rec.dehaze = v,
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
            rec.structure = 0.0;
            rec.dehaze = 0.0;
            rec.sharpen = 0.0;
            rec.noise_reduction = 0.0;
        });
    });

    let fx_neutral = r.vignette.abs() < 0.005 && r.grain.abs() < 0.005;
    let fx_controls = col()
        .w_full()
        .gap(space::MD)
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
        .gap(space::LG)
        .child(section_card(
            k,
            "sec-detail",
            "DETAIL & ATMOSPHERE",
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

#[allow(clippy::too_many_arguments)]
fn crop_pill_btn(
    k: &Kit,
    key: impl Into<String>,
    role: &'static str,
    aria_label: impl Into<String>,
    text_label: &'static str,
    height: f32,
    active: bool,
    rest_text_color: Color,
    on_press: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    let mut pill = row()
        .key(key.into())
        .role(role)
        .label(aria_label.into())
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .grow(1.0)
        .min_w(0.0)
        .h(height)
        .center()
        .radius(k.radius.sm)
        .fill(color::pill_fill(k, active))
        .hover(k.c.hover)
        .border(
            if active {
                border::REGULAR
            } else {
                border::HAIRLINE
            },
            color::pill_border(k, active),
        )
        .child(
            text(text_label)
                .font(ty::section_header(k))
                .color(if active {
                    k.c.text_accent
                } else {
                    rest_text_color
                }),
        )
        .on_press_tx(on_press);
    if role == "radio" {
        pill = pill.checked(active);
    }
    pill
}

fn crop_tab_content(s: &AppState, k: &Kit) -> El {
    let geo = s.recipe.geometry;
    let [out_w, out_h] = geo.cropped_output_size(s.image.width, s.image.height);

    let mut aspect_rows = col().w_full().gap(space::TIGHT);
    for chunk in CropAspect::ALL.chunks(4) {
        let mut r = row().w_full().gap(space::TIGHT);
        for &preset in chunk {
            let active = geo.aspect_preset == preset;
            r = r.child(crop_pill_btn(
                k,
                format!("aspect-{}", preset.label()),
                "radio",
                format!("Crop aspect {}", preset.label()),
                preset.label(),
                size::ASPECT_PILL_H,
                active,
                k.c.text_muted,
                move |s, tx| s.set_crop_aspect_tx(preset, tx),
            ));
        }
        aspect_rows = aspect_rows.child(r);
    }

    let crop_toggle = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            col()
                .gap(space::MICRO)
                .child(
                    text("Interactive Crop Frame")
                        .font(ty::control_label(k))
                        .color(k.c.text_base),
                )
                .child(
                    text(format!("Output {out_w}×{out_h} px"))
                        .font(ty::mono_compact(k))
                        .color(k.c.text_muted),
                ),
        )
        .child(
            Switch::new(s.camera.crop_mode)
                .label("Interactive Crop Frame")
                .hide_label()
                .on_toggle_tx(|s: &mut AppState, tx| s.toggle_crop_mode_tx(tx))
                .build(k),
        );

    let crop_reset = mini_reset_btn(k, "Reset Crop & Geometry", geo.is_identity(), |s, tx| {
        s.update_geometry_slider_tx(true, tx, |rec| rec.geometry.reset());
    });

    let crop_body = col()
        .w_full()
        .gap(space::MD)
        .child(crop_toggle)
        .child(aspect_rows)
        .child(
            text("Drag corners or edges on photo  •  Press C to apply")
                .font(ty::caption(k))
                .color(k.c.text_faint),
        );

    let straighten = develop_slider_inner(
        k,
        "sl-straighten",
        "Straighten",
        None,
        geo.sanitized_straighten_deg(),
        -45.0..=45.0,
        format!("{:+.1}°", geo.sanitized_straighten_deg()),
        0.0,
        false,
        |rec, v| rec.geometry.straighten_deg = v,
    );

    let rot_btn = |key: &'static str,
                   label: &'static str,
                   active: bool,
                   on_press: fn(&mut AppState, &mut Tx)| {
        crop_pill_btn(
            k,
            key,
            "button",
            label,
            label,
            size::ACTION_PILL_H,
            active,
            k.c.text_base,
            on_press,
        )
    };

    let xform_row = row()
        .w_full()
        .gap(space::TIGHT)
        .child(rot_btn("btn-rot-ccw", "↺ 90°", false, |s, tx| {
            s.update_geometry_slider_tx(true, tx, |r| {
                r.geometry.rotate_90 = (r.geometry.rotate_90 - 1).rem_euclid(4);
            });
        }))
        .child(rot_btn("btn-rot-cw", "↻ 90°", false, |s, tx| {
            s.update_geometry_slider_tx(true, tx, |r| {
                r.geometry.rotate_90 = (r.geometry.rotate_90 + 1).rem_euclid(4);
            });
        }))
        .child(rot_btn("btn-flip-h", "Flip H", geo.flip_h, |s, tx| {
            s.update_geometry_slider_tx(true, tx, |r| r.geometry.flip_h = !r.geometry.flip_h);
        }))
        .child(rot_btn("btn-flip-v", "Flip V", geo.flip_v, |s, tx| {
            s.update_geometry_slider_tx(true, tx, |r| r.geometry.flip_v = !r.geometry.flip_v);
        }));

    let rot_body = col()
        .w_full()
        .gap(space::MD)
        .child(straighten)
        .child(xform_row);

    let grid_idx = CompositionGrid::ALL
        .iter()
        .position(|&g| g == geo.grid)
        .unwrap_or(1);
    let grid_labels = ["Off", "Thirds", "Golden", "Diag"];
    let grid_seg = SegmentedControl::new(&grid_labels, Some(grid_idx))
        .label("Composition grid overlay")
        .width(None)
        .on_pick(|s: &mut AppState, i, tx| {
            let next_grid = CompositionGrid::ALL
                .get(i)
                .copied()
                .unwrap_or(CompositionGrid::RuleOfThirds);
            s.update_geometry_slider_tx(true, tx, |r| r.geometry.grid = next_grid);
        })
        .build(k);

    col()
        .w_full()
        .gap(space::LG)
        .child(section_card(
            k,
            "sec-crop",
            "CROP & ASPECT",
            Some(crop_reset),
            crop_body,
        ))
        .child(section_card(
            k,
            "sec-rotate",
            "ROTATE & FLIP",
            None,
            rot_body,
        ))
        .child(section_card(
            k,
            "sec-grid",
            "COMPOSITION GRID",
            None,
            grid_seg,
        ))
}

fn presets_tab_content(s: &AppState, k: &Kit) -> El {
    let mut grid_rows = col().w_full().gap(space::SM);
    for chunk in s.presets.chunks(2) {
        let mut r = row().w_full().gap(space::SM);
        for preset in chunk {
            let active = s.recipe.preset_name.as_deref() == Some(preset.name);
            let id = preset.id;
            let name = preset.name;
            let subtitle = if preset.recipe.bw.enabled || preset.recipe.saturation <= -0.95 {
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
                .focus_ring(
                    border::FOCUS_RING,
                    border::FOCUS_OFFSET_TIGHT,
                    k.c.border_focus,
                )
                .cursor(Cursor::Pointer)
                .grow(1.0)
                .min_w(0.0)
                .gap(space::XXS)
                .padding(space::LG)
                .radius(k.radius.md)
                .fill(color::pill_fill(k, active))
                .hover(k.c.hover)
                .border(
                    if active {
                        border::REGULAR
                    } else {
                        border::HAIRLINE
                    },
                    color::pill_border(k, active),
                )
                .child(
                    text(name)
                        .font(ty::control_label_medium(k))
                        .color(if active {
                            k.c.text_accent
                        } else {
                            k.c.text_base
                        }),
                )
                .child(text(subtitle).font(ty::caption(k)).color(k.c.text_faint))
                .on_press_tx(move |s: &mut AppState, tx| s.apply_preset_tx(id, tx));

            r = r.child(card);
        }
        grid_rows = grid_rows.child(r);
    }

    let clipboard_actions = row()
        .w_full()
        .gap(space::MD)
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

    let reset_all_row = row().w_full().gap(space::MD).child(
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
        .gap(space::LG)
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
                .gap(space::MD)
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
        .controls(4, "inspector-tabpanel")
        .on_pick(|s: &mut AppState, i, _| {
            s.active_tab = InspectorTab::from_index(i);
        })
        .build(k);

    let pinned_header = col()
        .key("inspector-pinned")
        .w_full()
        .gap(space::MD)
        .padding(space::LG)
        .border_sides(
            [border::NONE, border::NONE, border::REGULAR, border::NONE],
            k.c.border_base,
        )
        .child(histogram_panel(s, k))
        .child(tabs);

    let tab_body = match s.active_tab {
        InspectorTab::Light => light_tab_content(s, k),
        InspectorTab::Color => color_tab_content(s, k),
        InspectorTab::Detail => detail_tab_content(s, k),
        InspectorTab::Crop => crop_tab_content(s, k),
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
        .padding(space::LG)
        .child(tab_body);

    let scroll_viewport = el()
        .key("inspector-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .clip_fade(0.0, 0.0, 0.0, size::SCROLL_FADE)
        .child(scroll_content);

    col()
        .key("develop-inspector")
        .w(size::INSPECTOR_W)
        .h_full()
        .min_h(0.0)
        .fill(k.c.bg_base)
        .border_sides(
            [border::NONE, border::NONE, border::NONE, border::REGULAR],
            k.c.border_base,
        )
        .child(pinned_header)
        .child(k.scroll_bar(scroll_viewport))
}
