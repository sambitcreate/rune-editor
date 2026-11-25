//! Rune Darkroom — GPU-accelerated Capture One / Lightroom-style photo editor built on Rune.
//!
//! Run interactively:
//!   cargo run -p rune-editor --release
//!
//! Render headless verification snapshots:
//!   cargo run -p rune-editor -- --snap darkroom.png --scene warm-film
//!   cargo run -p rune-editor -- --snap split.png --scene split

mod compositor;
mod state;

use rune::file_dialog::FileDialogOptions;
use rune::tok::{self, radius};
use rune::{
    App, AppSpec, Color, Cursor, Cx, Drag, El, Key, KeyEvent, MagnifyEvent, Phase, RotateEvent,
    ScrollSpec, TextStyle, Tx, WheelEvent, col, effect, el, row, text,
};
use rune_editor_core::HISTOGRAM_BINS;
use rune_kit::{ControlSize, Kit, Slider};

use crate::compositor::DarkroomCompositor;
use crate::state::{AppState, PHOTO_VIEWPORT_MARK};

// Calibrated darkroom studio palette in linear light (Capture One / Lightroom neutral obsidian)
const BG_WINDOW: Color = Color { r: 0.008, g: 0.0085, b: 0.0095, a: 1.0 };
const BG_PANEL: Color = Color { r: 0.014, g: 0.015, b: 0.017, a: 1.0 };
const BG_CARD: Color = Color { r: 0.024, g: 0.026, b: 0.030, a: 1.0 };
const BG_CARD_HOVER: Color = Color { r: 0.042, g: 0.045, b: 0.052, a: 1.0 };
const BG_HISTOGRAM: Color = Color { r: 0.009, g: 0.010, b: 0.012, a: 1.0 };
const BORDER_SUBTLE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.07 };
const BORDER_ACCENT: Color = Color { r: 0.90, g: 0.44, b: 0.08, a: 0.85 };
const ACCENT_AMBER: Color = Color { r: 0.92, g: 0.48, b: 0.10, a: 1.0 };
const TEXT_PRIMARY: Color = Color { r: 0.86, g: 0.88, b: 0.91, a: 1.0 };
const TEXT_MUTED: Color = Color { r: 0.34, g: 0.37, b: 0.42, a: 1.0 };
const TEXT_DIM: Color = Color { r: 0.20, g: 0.22, b: 0.26, a: 1.0 };

const FONT_TITLE: TextStyle = TextStyle::new(13.0, 600.0);
const FONT_SECTION: TextStyle = TextStyle::new(11.0, 600.0);
const FONT_LABEL: TextStyle = TextStyle::new(12.0, 500.0);
const FONT_VALUE: TextStyle = TextStyle::new(11.0, 500.0).features("'tnum' 1");
const FONT_MICRO: TextStyle = TextStyle::new(10.5, 500.0).features("'tnum' 1");

fn toolbar_button(
    key: &'static str,
    label: &str,
    active: bool,
    on_press: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .px(10.0)
        .py(5.0)
        .radius(radius::SM)
        .fill(if active { ACCENT_AMBER.alpha(0.20) } else { BG_CARD })
        .hover(if active { ACCENT_AMBER.alpha(0.28) } else { BG_CARD_HOVER })
        .border(1.0, if active { BORDER_ACCENT } else { BORDER_SUBTLE })
        .child(
            text(label)
                .font(FONT_LABEL)
                .color(if active { ACCENT_AMBER } else { TEXT_PRIMARY }),
        )
        .on_press_tx(on_press)
}

fn trigger_open_dialog(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Open Photo".into(),
        ..Default::default()
    }
    .filter("Images", &["jpg", "jpeg", "png", "tif", "tiff", "webp"]);

    tx.open_file(options, |s: &mut AppState, result, _| match result {
        Ok(Some(path)) => s.open_image_path(path),
        Ok(None) => {}
        Err(err) => s.status_message = Some(format!("Open dialog error: {err}")),
    });
}

fn top_toolbar(s: &AppState) -> El {
    let left = row()
        .items_center()
        .gap(10.0)
        .child(
            row()
                .items_center()
                .gap(7.0)
                .child(
                    el().size(9.0, 9.0)
                        .radius(4.5)
                        .fill(ACCENT_AMBER),
                )
                .child(text("RUNE DARKROOM").font(FONT_TITLE).color(TEXT_PRIMARY)),
        )
        .child(el().w(1.0).h(16.0).fill(BORDER_SUBTLE))
        .child(
            text(s.image.display_name.as_str())
                .font(FONT_LABEL)
                .color(TEXT_MUTED)
                .truncate(),
        );

    let center = row()
        .items_center()
        .gap(6.0)
        .child(toolbar_button("btn-fit", "Fit", (s.camera.zoom - 1.0).abs() < 0.02, |s, _| {
            s.camera.reset();
        }))
        .child(toolbar_button("btn-100", "200%", (s.camera.zoom - 2.0).abs() < 0.05, |s, _| {
            s.camera.zoom = 2.0;
        }))
        .child(toolbar_button(
            "btn-split",
            "Before / After",
            s.camera.split_compare,
            |s, _| {
                s.camera.split_compare = !s.camera.split_compare;
            },
        ));

    let right = row()
        .items_center()
        .gap(6.0)
        .child(toolbar_button("btn-open", "Open…", false, |_, tx| {
            trigger_open_dialog(tx);
        }))
        .child(toolbar_button("btn-undo", "Undo", false, |s, _| s.undo()))
        .child(toolbar_button("btn-redo", "Redo", false, |s, _| s.redo()))
        .child(toolbar_button("btn-save", "Save Sidecar", false, |s, _| s.save_sidecar()))
        .child(toolbar_button("btn-reset", "Reset", false, |s, _| s.reset_recipe()));

    row()
        .key("toolbar")
        .w_full()
        .h(46.0)
        .px(14.0)
        .items_center()
        .justify_between()
        .fill(BG_PANEL)
        .border_sides([0.0, 0.0, 1.0, 0.0], BORDER_SUBTLE)
        .child(left)
        .child(center)
        .child(right)
}

fn histogram_panel(s: &AppState) -> El {
    let hist_h = 68.0_f32;
    let mut bars = row()
        .key("histogram-bars")
        .w_full()
        .h(hist_h)
        .items_end()
        .gap(1.0);

    for i in 0..HISTOGRAM_BINS {
        let r_h = (s.histogram.r[i] * hist_h).max(1.0);
        let g_h = (s.histogram.g[i] * hist_h).max(1.0);
        let b_h = (s.histogram.b[i] * hist_h).max(1.0);
        let l_h = (s.histogram.luma[i] * hist_h).max(1.0);

        let bin = el()
            .grow(1.0)
            .h(hist_h)
            .child(
                el().absolute(0.0, hist_h - l_h)
                    .w_full()
                    .h(l_h)
                    .fill(Color { r: 0.78, g: 0.80, b: 0.84, a: 0.22 }),
            )
            .child(
                el().absolute(0.0, hist_h - r_h)
                    .w_full()
                    .h(r_h)
                    .fill(Color { r: 0.96, g: 0.32, b: 0.32, a: 0.42 }),
            )
            .child(
                el().absolute(0.0, hist_h - g_h)
                    .w_full()
                    .h(g_h)
                    .fill(Color { r: 0.28, g: 0.88, b: 0.46, a: 0.42 }),
            )
            .child(
                el().absolute(0.0, hist_h - b_h)
                    .w_full()
                    .h(b_h)
                    .fill(Color { r: 0.30, g: 0.56, b: 0.98, a: 0.45 }),
            );
        bars = bars.child(bin);
    }

    let header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(text("HISTOGRAM").font(FONT_SECTION).color(TEXT_MUTED))
        .child(
            text(format!(
                "BLK {:.1}%  •  WHT {:.1}%",
                s.histogram.shadow_clip_pct, s.histogram.highlight_clip_pct
            ))
            .font(FONT_MICRO)
            .color(TEXT_DIM),
        );

    col()
        .key("histogram-card")
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .radius(radius::MD)
        .fill(BG_HISTOGRAM)
        .border(1.0, BORDER_SUBTLE)
        .child(header)
        .child(bars)
}

fn presets_section(s: &AppState) -> El {
    let mut rows = col().w_full().gap(6.0);
    for chunk in s.presets.chunks(3) {
        let mut r = row().w_full().gap(6.0);
        for preset in chunk {
            let active = s.recipe.preset_name.as_deref() == Some(preset.name);
            let id = preset.id;
            let name = preset.name;
            r = r.child(
                el().key(format!("preset-{id}"))
                    .role("button")
                    .label(name)
                    .focusable(true)
                    .cursor(Cursor::Pointer)
                    .px(9.0)
                    .py(5.0)
                    .radius(radius::SM)
                    .fill(if active { ACCENT_AMBER.alpha(0.22) } else { BG_CARD })
                    .hover(if active { ACCENT_AMBER.alpha(0.30) } else { BG_CARD_HOVER })
                    .border(1.0, if active { BORDER_ACCENT } else { BORDER_SUBTLE })
                    .child(
                        text(name)
                            .font(FONT_MICRO)
                            .color(if active { ACCENT_AMBER } else { TEXT_PRIMARY }),
                    )
                    .on_press(move |s: &mut AppState| s.apply_preset(id)),
            );
        }
        rows = rows.child(r);
    }

    col()
        .key("presets-section")
        .w_full()
        .gap(8.0)
        .child(text("STYLES & PRESETS").font(FONT_SECTION).color(TEXT_MUTED))
        .child(rows)
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
    setter: fn(&mut rune_editor_core::DevelopRecipe, f32),
) -> El {
    let header = row()
        .w_full()
        .justify_between()
        .items_center()
        .child(
            el().role("button")
                .label(format!("Reset {label}"))
                .cursor(Cursor::Pointer)
                .child(text(label).font(FONT_LABEL).color(TEXT_PRIMARY))
                .on_press(move |s: &mut AppState| {
                    s.update_slider(true, |r| setter(r, reset_val));
                }),
        )
        .child(
            text(&value_fmt)
                .font(FONT_VALUE)
                .color(if (value - reset_val).abs() > 0.005 {
                    ACCENT_AMBER
                } else {
                    TEXT_MUTED
                }),
        );

    let slider = Slider::new(format!("{key}-slider"), value)
        .range(range)
        .label(label)
        .hide_label()
        .value_text(value_fmt)
        .control_size(ControlSize::Small)
        .width(264.0)
        .on_change(move |s: &mut AppState, v: f32, done: bool| {
            s.update_slider(done, |r| setter(r, v));
        })
        .build(k);

    col().key(key).w_full().gap(4.0).child(header).child(slider)
}

fn section_card(title: &'static str, content: El) -> El {
    col()
        .w_full()
        .gap(10.0)
        .padding(12.0)
        .radius(radius::MD)
        .fill(BG_CARD.alpha(0.55))
        .border(1.0, BORDER_SUBTLE)
        .child(text(title).font(FONT_SECTION).color(TEXT_MUTED))
        .child(content)
}

fn right_inspector(s: &AppState) -> El {
    let k = Kit::neutral(true);
    let r = &s.recipe;

    let wb_controls = col()
        .w_full()
        .gap(10.0)
        .child(develop_slider(
            &k,
            "sl-temp",
            "Temperature",
            r.temperature,
            -1.0..=1.0,
            format!("{:+.0}", r.temperature * 100.0),
            0.0,
            |rec, v| rec.temperature = v,
        ))
        .child(develop_slider(
            &k,
            "sl-tint",
            "Tint",
            r.tint,
            -1.0..=1.0,
            format!("{:+.0}", r.tint * 100.0),
            0.0,
            |rec, v| rec.tint = v,
        ));

    let tone_controls = col()
        .w_full()
        .gap(10.0)
        .child(develop_slider(
            &k,
            "sl-exposure",
            "Exposure",
            r.exposure,
            -4.0..=4.0,
            format!("{:+.2} EV", r.exposure),
            0.0,
            |rec, v| rec.exposure = v,
        ))
        .child(develop_slider(
            &k,
            "sl-contrast",
            "Contrast",
            r.contrast,
            -1.0..=1.0,
            format!("{:+.0}", r.contrast * 100.0),
            0.0,
            |rec, v| rec.contrast = v,
        ))
        .child(develop_slider(
            &k,
            "sl-highlights",
            "Highlights",
            r.highlights,
            -1.0..=1.0,
            format!("{:+.0}", r.highlights * 100.0),
            0.0,
            |rec, v| rec.highlights = v,
        ))
        .child(develop_slider(
            &k,
            "sl-shadows",
            "Shadows",
            r.shadows,
            -1.0..=1.0,
            format!("{:+.0}", r.shadows * 100.0),
            0.0,
            |rec, v| rec.shadows = v,
        ))
        .child(develop_slider(
            &k,
            "sl-whites",
            "Whites",
            r.whites,
            -1.0..=1.0,
            format!("{:+.0}", r.whites * 100.0),
            0.0,
            |rec, v| rec.whites = v,
        ))
        .child(develop_slider(
            &k,
            "sl-blacks",
            "Blacks",
            r.blacks,
            -1.0..=1.0,
            format!("{:+.0}", r.blacks * 100.0),
            0.0,
            |rec, v| rec.blacks = v,
        ));

    let color_controls = col()
        .w_full()
        .gap(10.0)
        .child(develop_slider(
            &k,
            "sl-vibrance",
            "Vibrance",
            r.vibrance,
            -1.0..=1.0,
            format!("{:+.0}", r.vibrance * 100.0),
            0.0,
            |rec, v| rec.vibrance = v,
        ))
        .child(develop_slider(
            &k,
            "sl-saturation",
            "Saturation",
            r.saturation,
            -1.0..=1.0,
            format!("{:+.0}", r.saturation * 100.0),
            0.0,
            |rec, v| rec.saturation = v,
        ));

    let fx_controls = col()
        .w_full()
        .gap(10.0)
        .child(develop_slider(
            &k,
            "sl-vignette",
            "Vignette",
            r.vignette,
            -1.0..=1.0,
            format!("{:+.0}", r.vignette * 100.0),
            0.0,
            |rec, v| rec.vignette = v,
        ))
        .child(develop_slider(
            &k,
            "sl-grain",
            "Film Grain",
            r.grain,
            0.0..=1.0,
            format!("{:.0}", r.grain * 100.0),
            0.0,
            |rec, v| rec.grain = v,
        ));

    let scroll_body = col()
        .w(316.0)
        .gap(12.0)
        .padding(14.0)
        .child(histogram_panel(s))
        .child(presets_section(s))
        .child(section_card("WHITE BALANCE", wb_controls))
        .child(section_card("LIGHT & TONE", tone_controls))
        .child(section_card("COLOR", color_controls))
        .child(section_card("EFFECTS", fx_controls));

    col()
        .key("develop-inspector")
        .w(316.0)
        .h_full()
        .min_h(0.0)
        .fill(BG_PANEL)
        .border_sides([0.0, 0.0, 0.0, 1.0], BORDER_SUBTLE)
        .child(
            el().key("inspector-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec::default())
                .child(scroll_body),
        )
}

fn photo_viewport(s: &AppState) -> El {
    let gpu_slot = effect(PHOTO_VIEWPORT_MARK)
        .key("photo-gpu-slot")
        .w_full()
        .h_full()
        .cursor(if s.camera.zoom > 1.01 {
            Cursor::Grab
        } else {
            Cursor::Default
        })
        .on_magnify(|s: &mut AppState, ev: MagnifyEvent, _| {
            s.camera.zoom_by((1.0 + ev.delta).clamp(0.5, 2.0));
            true
        })
        .on_rotate(|s: &mut AppState, ev: RotateEvent, _| {
            s.camera.rotation_deg = (s.camera.rotation_deg + ev.degrees).clamp(-180.0, 180.0);
            true
        })
        .on_smart_magnify(|s: &mut AppState, _, _, _| {
            if (s.camera.zoom - 1.0).abs() < 0.05 {
                s.camera.zoom = 2.0;
            } else {
                s.camera.reset();
            }
            true
        })
        .on_wheel(|s: &mut AppState, ev: WheelEvent, _| {
            if ev.mods.cmd || ev.mods.ctrl {
                let factor = (1.0 + ev.dy * 0.005).clamp(0.8, 1.25);
                s.camera.zoom_by(factor);
            } else if s.camera.zoom > 1.01 {
                s.camera.pan_by(ev.dx * 0.0025, ev.dy * 0.0025);
            } else {
                return false;
            }
            true
        })
        .on_drag(|s: &mut AppState, d: &mut Drag, _| {
            match d.phase {
                Phase::Down => {
                    s.last_drag_pos = Some((d.x, d.y));
                    if s.camera.split_compare && d.w > 1.0 {
                        let frac = (d.x / d.w).clamp(0.05, 0.95);
                        if (frac - s.camera.split_pos).abs() < 0.08 {
                            s.camera.split_pos = frac;
                        }
                    }
                }
                Phase::Move => {
                    if s.camera.split_compare && d.w > 1.0 && s.camera.zoom <= 1.02 {
                        s.camera.split_pos = (d.x / d.w).clamp(0.05, 0.95);
                    } else if let Some((lx, ly)) = s.last_drag_pos {
                        let dx = (d.x - lx) / d.w.max(1.0) * 2.0;
                        let dy = (d.y - ly) / d.h.max(1.0) * 2.0;
                        s.camera.pan_by(dx, dy);
                    }
                    s.last_drag_pos = Some((d.x, d.y));
                }
                Phase::Up => {
                    s.last_drag_pos = None;
                }
            }
        });

    let mut container = el()
        .key("viewport-container")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .fill(BG_WINDOW)
        .border(
            if s.drop_hover { 2.0 } else { 0.0 },
            if s.drop_hover { ACCENT_AMBER } else { Color::TRANSPARENT },
        )
        .on_drop_hover_within(|s: &mut AppState, over, _| {
            s.drop_hover = over;
        })
        .on_drop_files(|s: &mut AppState, files, _| {
            s.drop_hover = false;
            if let Some(first) = files.first() {
                s.open_image_path(first.clone());
            }
        })
        .child(gpu_slot);

    // Floating Before/After badges when Split Compare is active
    if s.camera.split_compare {
        container = container
            .child(
                el().pin(Some(18.0), Some(16.0), None, None)
                    .px(9.0)
                    .py(4.0)
                    .radius(radius::SM)
                    .fill(Color { r: 0.01, g: 0.01, b: 0.012, a: 0.82 })
                    .border(1.0, BORDER_SUBTLE)
                    .child(text("BEFORE").font(FONT_MICRO).color(TEXT_MUTED)),
            )
            .child(
                el().pin(None, Some(16.0), Some(18.0), None)
                    .px(9.0)
                    .py(4.0)
                    .radius(radius::SM)
                    .fill(Color { r: 0.01, g: 0.01, b: 0.012, a: 0.82 })
                    .border(1.0, BORDER_ACCENT)
                    .child(text("AFTER").font(FONT_MICRO).color(ACCENT_AMBER)),
            );
    }

    container
}

fn status_bar(s: &AppState) -> El {
    let zoom_pct = (s.camera.zoom * 100.0).round() as i32;
    let rot_str = if s.camera.rotation_deg.abs() > 0.1 {
        format!("  •  {:.1}°", s.camera.rotation_deg)
    } else {
        String::new()
    };

    let left = row()
        .items_center()
        .gap(10.0)
        .child(
            text(format!(
                "{}×{}  •  {}",
                s.image.width, s.image.height, s.image.format_label
            ))
            .font(FONT_MICRO)
            .color(TEXT_MUTED),
        )
        .child(
            text(s.status_message.as_deref().unwrap_or(""))
                .font(FONT_MICRO)
                .color(TEXT_DIM),
        );

    let right = text(format!(
        "Zoom {zoom_pct}%{rot_str}  •  \\ Compare  •  ⌘Z Undo"
    ))
    .font(FONT_MICRO)
    .color(TEXT_DIM);

    row()
        .key("status-bar")
        .w_full()
        .h(28.0)
        .px(14.0)
        .items_center()
        .justify_between()
        .fill(BG_PANEL)
        .border_sides([1.0, 0.0, 0.0, 0.0], BORDER_SUBTLE)
        .child(left)
        .child(right)
}

fn handle_key(s: &mut AppState, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let cmd = ev.mods.cmd || ev.mods.ctrl;
    match &ev.key {
        Key::Text(ch) if ch == "\\" => {
            s.camera.split_compare = !s.camera.split_compare;
            true
        }
        Key::Text(ch) if cmd && ch.eq_ignore_ascii_case("z") => {
            if ev.mods.shift {
                s.redo();
            } else {
                s.undo();
            }
            true
        }
        Key::Text(ch) if cmd && ch.eq_ignore_ascii_case("o") => {
            trigger_open_dialog(tx);
            true
        }
        Key::Text(ch) if cmd && ch.eq_ignore_ascii_case("s") => {
            s.save_sidecar();
            true
        }
        Key::Text(ch) if cmd && ch == "0" => {
            s.camera.reset();
            true
        }
        _ => false,
    }
}

fn view(s: &AppState, _cx: &mut Cx<AppState>) -> El {
    let _ = tok::theme(true);
    let body = row()
        .key("workspace-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(photo_viewport(s))
        .child(right_inspector(s));

    col()
        .key("root")
        .w_full()
        .h_full()
        .fill(BG_WINDOW)
        .on_key(handle_key)
        .child(top_toolbar(s))
        .child(body)
        .child(status_bar(s))
}

fn main() {
    let spec = AppSpec::<AppState> {
        scenes: &[
            ("warm-film", |s| s.apply_preset("warm_film")),
            ("split", |s| {
                s.apply_preset("moody_dusk");
                s.camera.split_compare = true;
                s.camera.split_pos = 0.5;
            }),
            ("editorial-bw", |s| s.apply_preset("editorial_bw")),
        ],
        ..AppSpec::new("Rune Darkroom", (1360.0, 860.0), AppState::default, view)
    };

    App::new(spec)
        .compositor(DarkroomCompositor::default())
        .on_open_files(|s, files, _| {
            if let Some(first) = files.first() {
                s.open_image_path(first.clone());
            }
        })
        .run();
}
