//! Dedicated Capture One-style Multi-Recipe Export view (`⌘⇧E`) with simultaneous
//! Process Recipes, Dynamic Naming Tokens + live filename preview, Text Watermarks,
//! Display P3 / sRGB color space selection, and EXIF metadata stripping.

use rune::file_dialog::FileDialogOptions;
use rune::{Cursor, Drag, El, Phase, ScrollSpec, Tx, col, el, icon, row, text};
use rune_editor_decode::{ExportColorSpace, ExportFormat, WatermarkPosition};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, Checkbox, ControlSize, Divider, Kit,
    SegmentedControl, Switch, TextInput,
};
use rune_theme_opencode::icons;

use crate::menu::trigger_export_dialog;
use crate::state::AppState;
use crate::tokens::{border, color, size, space, ty};

const NAMING_TOKENS: [&str; 6] = [
    "{ImageName}",
    "{Recipe}",
    "{Variant}",
    "{Date}",
    "{ISO}",
    "{Counter}",
];

/// Full-workspace Multi-Recipe Export view shown when `s.show_export` is `true`.
pub fn export_view(s: &AppState, k: &Kit) -> El {
    let enabled_count = s.export_recipes.iter().filter(|r| r.enabled).count();
    let [out_w, out_h] = s
        .recipe
        .geometry
        .cropped_output_size(s.image.width, s.image.height);

    let sidebar = export_sidebar(s, k, enabled_count, out_w, out_h);

    let content = col()
        .w_full()
        .max_w(size::EXPORT_CONTENT_MAX_W)
        .gap(space::XL)
        .child(recipes_card(s, k))
        .child(naming_card(s, k))
        .child(watermark_card(s, k))
        .child(color_and_metadata_card(s, k))
        .child(export_action_bar(s, k, enabled_count));

    let scroll_viewport = el()
        .key("export-scroll")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .child(
            col()
                .w_full()
                .px(space::MODAL_PAD)
                .py(space::MODAL_PAD)
                .items_center()
                .child(content),
        );

    row()
        .key("export-view")
        .role("region")
        .label("Multi-Recipe Export")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(k.c.page)
        .child(sidebar)
        .child(k.scroll_bar(scroll_viewport))
}

fn export_sidebar(s: &AppState, k: &Kit, enabled_count: usize, out_w: u32, out_h: u32) -> El {
    let back_btn = Button::new("← Back to Darkroom")
        .variant(ButtonVariant::Neutral)
        .control_size(ControlSize::Regular)
        .on_press_tx(|s: &mut AppState, tx| s.set_show_export_tx(false, tx))
        .build(k);

    let quick_single = Button::new("Quick Single File…")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .icon(&icons::SHARE)
        .on_press_tx(|s: &mut AppState, tx| trigger_export_dialog(s, tx))
        .build(k);

    col()
        .key("export-sidebar")
        .w(size::SETTINGS_SIDEBAR_W)
        .h_full()
        .px(space::XL)
        .py(space::XL)
        .gap(space::LG)
        .fill(k.c.bg_deep)
        .border_sides(
            [border::NONE, border::REGULAR, border::NONE, border::NONE],
            k.c.border_base,
        )
        .child(back_btn)
        .child(
            text("PROCESS RECIPES")
                .font(ty::section_header(k))
                .color(k.c.text_faint),
        )
        .child(
            col()
                .gap(space::SM)
                .padding(space::LG)
                .radius(k.radius.md)
                .fill(k.c.layer_01)
                .border(border::HAIRLINE, k.c.border_base)
                .child(
                    text(s.image.display_name.as_str())
                        .font(ty::control_label_medium(k))
                        .color(k.c.text_base)
                        .truncate(),
                )
                .child(
                    text(format!("Source Crop: {out_w}×{out_h} px"))
                        .font(ty::mono_compact(k))
                        .color(k.c.text_muted),
                )
                .child(
                    text(format!(
                        "Variant: V{} · {}",
                        s.active_variant_index() + 1,
                        s.export_color_space.label()
                    ))
                    .font(ty::mono_compact(k))
                    .color(k.c.text_muted),
                )
                .child(
                    Badge::new(format!("{enabled_count} Active Recipe(s)"))
                        .variant(if enabled_count > 0 {
                            BadgeVariant::Accent
                        } else {
                            BadgeVariant::Subtle
                        })
                        .build(k),
                ),
        )
        .child(quick_single)
        .child(el().grow(1.0))
        .child(
            col()
                .gap(space::XXS)
                .child(
                    text("Inviolable Originals")
                        .font(ty::mono_micro(k))
                        .color(k.c.text_faint),
                )
                .child(
                    text("Atomic write + guard_not_source")
                        .font(ty::mono_micro(k))
                        .color(k.c.text_faint),
                ),
        )
}

fn export_card(k: &Kit) -> El {
    col()
        .w_full()
        .px(space::XXL)
        .py(space::XL)
        .gap(space::MD)
        .radius(k.radius.lg)
        .continuous_corners()
        .fill(k.c.bg_base)
        .border(border::REGULAR, k.c.border_base)
}

fn format_summary(fmt: ExportFormat, max_edge: Option<u32>, sharpen: f32) -> String {
    let fmt_str = match fmt {
        ExportFormat::Jpeg { quality } => format!("JPEG {quality}%"),
        ExportFormat::Png8 => "PNG 8-bit".to_owned(),
        ExportFormat::Png16 => "PNG 16-bit".to_owned(),
        ExportFormat::Tiff16 => "TIFF 16-bit".to_owned(),
        ExportFormat::WebP => "WebP".to_owned(),
    };
    let size_str = match max_edge {
        Some(edge) => format!("{edge}px long edge"),
        None => "Full Resolution".to_owned(),
    };
    if sharpen > 0.01 {
        format!("{fmt_str} · {size_str} · Screen Sharpen")
    } else {
        format!("{fmt_str} · {size_str}")
    }
}

fn recipes_card(s: &AppState, k: &Kit) -> El {
    let mut rows = col().w_full().gap(space::SM);
    for (idx, spec) in s.export_recipes.iter().enumerate() {
        let preview = s.preview_export_filename(spec);
        let summary = format_summary(
            spec.options.format,
            spec.options.max_edge,
            spec.options.output_sharpen,
        );
        let enabled = spec.enabled;
        let item = row()
            .key(format!("export-recipe-row-{idx}"))
            .w_full()
            .items_center()
            .justify_between()
            .gap(space::MD)
            .px(space::LG)
            .py(space::MD)
            .radius(k.radius.md)
            .fill(if enabled { k.c.layer_01 } else { k.c.bg_deep })
            .border(
                border::HAIRLINE,
                if enabled {
                    k.c.border_strong
                } else {
                    k.c.border_muted
                },
            )
            .child(
                row()
                    .items_center()
                    .gap(space::MD)
                    .child(
                        Checkbox::new(enabled)
                            .label(format!("Enable {}", spec.name))
                            .hide_label()
                            .on_toggle_tx(move |s: &mut AppState, tx| {
                                if let Some(r) = s.export_recipes.get_mut(idx) {
                                    r.enabled = !r.enabled;
                                    s.sync_window_state(tx);
                                }
                            })
                            .build(k),
                    )
                    .child(
                        col()
                            .gap(space::MICRO)
                            .child(
                                text(spec.name.as_str())
                                    .font(ty::control_label_medium(k))
                                    .color(if enabled {
                                        k.c.text_base
                                    } else {
                                        k.c.text_muted
                                    }),
                            )
                            .child(text(summary).font(ty::caption(k)).color(k.c.text_faint)),
                    ),
            )
            .child(text(preview).font(ty::mono_compact(k)).color(if enabled {
                k.c.text_accent
            } else {
                k.c.text_faint
            }));
        rows = rows.child(item);
    }

    export_card(k)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    text("Simultaneous Export Recipes")
                        .font(ty::dialog_title(k))
                        .color(k.c.text_base),
                )
                .child(
                    text("Checked recipes export in one atomic pass")
                        .font(ty::caption(k))
                        .color(k.c.text_muted),
                ),
        )
        .child(Divider::horizontal().build(k))
        .child(rows)
}

fn naming_card(s: &AppState, k: &Kit) -> El {
    let preview_name = s
        .export_recipes
        .iter()
        .find(|r| r.enabled)
        .or_else(|| s.export_recipes.first())
        .map(|spec| s.preview_export_filename(spec))
        .unwrap_or_else(|| "Editorial_Portrait.jpg".to_owned());

    let mut chips = row().w_full().gap(space::XS);
    for token in NAMING_TOKENS {
        chips = chips.child(
            row()
                .key(format!("token-chip-{token}"))
                .role("button")
                .label(format!("Insert naming token {token}"))
                .cursor(Cursor::Pointer)
                .px(space::MD)
                .h(size::PILL_COMPACT_H)
                .center()
                .radius(k.radius.sm)
                .fill(k.c.layer_01)
                .hover(k.c.hover)
                .border(border::HAIRLINE, k.c.border_base)
                .child(text(token).font(ty::mono_micro(k)).color(k.c.text_muted))
                .on_press(move |s: &mut AppState| {
                    if !s.export_naming_edit.text.is_empty()
                        && !s.export_naming_edit.text.ends_with('_')
                    {
                        s.export_naming_edit.insert("_");
                    }
                    s.export_naming_edit.insert(token);
                }),
        );
    }

    export_card(k)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    text("Output Naming Template")
                        .font(ty::dialog_title(k))
                        .color(k.c.text_base),
                )
                .child(
                    text(format!("Preview: {preview_name}"))
                        .font(ty::mono_compact(k))
                        .color(k.c.text_accent),
                ),
        )
        .child(
            TextInput::new(&s.export_naming_edit)
                .placeholder("{ImageName}_{Recipe}")
                .width(None)
                .on_edit(|s: &mut AppState| &mut s.export_naming_edit)
                .build(k),
        )
        .child(chips)
}

fn watermark_card(s: &AppState, k: &Kit) -> El {
    let opacity_pct = (s.export_watermark_opacity * 100.0).round() as i32;
    let bar_w = 240.0_f32;
    let fill_w = (s.export_watermark_opacity.clamp(0.0, 1.0) * bar_w).max(0.0);

    let opacity_capsule = el()
        .key("sl-export-wm-opacity")
        .role("slider")
        .label("Watermark opacity")
        .cursor(Cursor::ResizeColumn)
        .w(bar_w)
        .h(size::CAPSULE_SLIDER_H)
        .radius(k.radius.lg)
        .fill(color::capsule_bg(k))
        .hover(color::capsule_hover(k))
        .border(border::HAIRLINE, k.c.border_base)
        .clip_rounded(k.radius.lg)
        .child(
            el().absolute(0.0, 0.0)
                .size(fill_w, size::CAPSULE_SLIDER_H)
                .fill(color::capsule_fill(k))
                .a11y_hidden(),
        )
        .child(
            row()
                .w_full()
                .h_full()
                .px(space::LG)
                .items_center()
                .justify_between()
                .child(
                    text("Opacity")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                )
                .child(
                    text(format!("{opacity_pct}%"))
                        .font(ty::mono_value(k))
                        .color(k.c.text_base),
                ),
        )
        .on_drag(|s: &mut AppState, d: &mut Drag, _| {
            if matches!(d.phase, Phase::Down | Phase::Move | Phase::Up) {
                s.export_watermark_opacity = (d.x / d.w.max(1.0)).clamp(0.0, 1.0);
            }
        });

    let mut pos_row = row().w_full().gap(space::XS);
    for pos in WatermarkPosition::ALL {
        let active = s.export_watermark_position == pos;
        pos_row = pos_row.child(
            row()
                .key(format!("wm-pos-{}", pos.cli_slug()))
                .role("radio")
                .label(format!("Watermark position {}", pos.label()))
                .checked(active)
                .cursor(Cursor::Pointer)
                .grow(1.0)
                .min_w(0.0)
                .h(size::PILL_H)
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
                    text(pos.label())
                        .font(ty::section_header(k))
                        .color(if active {
                            k.c.text_accent
                        } else {
                            k.c.text_muted
                        }),
                )
                .on_press(move |s: &mut AppState| {
                    s.export_watermark_position = pos;
                }),
        );
    }

    let mut card = export_card(k).child(
        row()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                col()
                    .gap(space::MICRO)
                    .child(
                        text("Text Watermark")
                            .font(ty::dialog_title(k))
                            .color(k.c.text_base),
                    )
                    .child(
                        text("Composite anti-aliased branding onto exported files")
                            .font(ty::caption(k))
                            .color(k.c.text_muted),
                    ),
            )
            .child(
                Switch::new(s.export_watermark_enabled)
                    .label("Enable text watermark")
                    .hide_label()
                    .on_toggle(|s: &mut AppState| {
                        s.export_watermark_enabled = !s.export_watermark_enabled;
                    })
                    .build(k),
            ),
    );

    if s.export_watermark_enabled {
        card = card
            .child(Divider::horizontal().build(k))
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(space::MD)
                    .child(
                        el().grow(1.0).min_w(0.0).child(
                            TextInput::new(&s.export_watermark_edit)
                                .placeholder("© Photographer Name")
                                .width(None)
                                .on_edit(|s: &mut AppState| &mut s.export_watermark_edit)
                                .build(k),
                        ),
                    )
                    .child(opacity_capsule),
            )
            .child(pos_row);
    }
    card
}

fn color_and_metadata_card(s: &AppState, k: &Kit) -> El {
    let cs_idx = match s.export_color_space {
        ExportColorSpace::Srgb => 0,
        ExportColorSpace::DisplayP3 => 1,
    };
    let cs_seg = SegmentedControl::new(&["sRGB", "Display P3"], Some(cs_idx))
        .label("Export color space")
        .fit()
        .on_pick(|s: &mut AppState, i, _| {
            s.export_color_space = if i == 1 {
                ExportColorSpace::DisplayP3
            } else {
                ExportColorSpace::Srgb
            };
        })
        .build(k);

    export_card(k)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    col()
                        .gap(space::MICRO)
                        .child(
                            text("Output Color Space")
                                .font(ty::control_label_medium(k))
                                .color(k.c.text_base),
                        )
                        .child(
                            text("Convert linear scene-referred color to sRGB or wide-gamut Display P3")
                                .font(ty::caption(k))
                                .color(k.c.text_muted),
                        ),
                )
                .child(cs_seg),
        )
        .child(Divider::horizontal().build(k))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    col()
                        .gap(space::MICRO)
                        .child(
                            text("Strip EXIF & Camera Metadata")
                                .font(ty::control_label_medium(k))
                                .color(k.c.text_base),
                        )
                        .child(
                            text("Remove camera, lens, and GPS metadata from exported deliverables")
                                .font(ty::caption(k))
                                .color(k.c.text_muted),
                        ),
                )
                .child(
                    Switch::new(s.export_strip_metadata)
                        .label("Strip EXIF metadata")
                        .hide_label()
                        .on_toggle(|s: &mut AppState| {
                            s.export_strip_metadata = !s.export_strip_metadata;
                        })
                        .build(k),
                ),
        )
}

fn trigger_multi_recipe_folder_export(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Choose Export Output Folder".into(),
        ..Default::default()
    };
    tx.open_folder(options, |s: &mut AppState, result, tx| match result {
        Ok(Some(dir)) => s.run_multi_recipe_export_tx(dir, tx),
        Ok(None) => {}
        Err(err) => s.status_message = Some(format!("Export folder dialog error: {err}")),
    });
}

fn export_action_bar(s: &AppState, k: &Kit, enabled_count: usize) -> El {
    let dest_label = s
        .export_output_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Select output folder on export…".to_owned());

    row()
        .w_full()
        .items_center()
        .justify_between()
        .gap(space::MD)
        .px(space::XXL)
        .py(space::LG)
        .radius(k.radius.lg)
        .fill(k.c.bg_base)
        .border(border::REGULAR, k.c.border_base)
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .min_w(0.0)
                .shrink()
                .child(
                    icon(&icons::FOLDER, size::ICON_MD)
                        .color(k.c.icon_muted)
                        .a11y_hidden(),
                )
                .child(
                    text(dest_label)
                        .font(ty::mono_compact(k))
                        .color(k.c.text_muted)
                        .truncate(),
                ),
        )
        .child(
            Button::new(format!("Export {enabled_count} Recipe(s)…"))
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Regular)
                .icon(&icons::SHARE)
                .disabled(enabled_count == 0)
                .on_press_tx(|_: &mut AppState, tx| trigger_multi_recipe_folder_export(tx))
                .build(k),
        )
}
