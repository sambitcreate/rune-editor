//! Collapsible bottom Filmstrip & Folder Culling bar (`F` or toolbar toggle).
//!
//! Renders high-speed `rune::Pixels` thumbnail cards with star ratings (`1`–`5`),
//! Pick (`P`) / Reject (`X`) badges, filter pills (`All | Picks | ★+`), and instant
//! keyboard (`←`/`→`) or click switching.

use rune::{Color, Cursor, El, SurfaceFit, col, el, icon, row, text};
use rune_editor_core::{ColorTag, CullFilter, CullFlag};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit,
    SegmentedControl,
};
use rune_theme_opencode::icons;

use crate::menu::trigger_open_folder;
use crate::state::AppState;
use crate::tokens::{border, color, radius, size, space, ty};

pub fn filmstrip_bar(s: &AppState, k: &Kit) -> El {
    let (total, picks, _rejects) = s.catalog.counts();
    let visible = s.catalog.filtered_indices();

    let title_str = if let Some(folder) = &s.catalog.folder {
        folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Folder".to_owned())
    } else {
        "Demo Session".to_owned()
    };

    let filter_sel = match s.catalog.filter {
        CullFilter::All => Some(0),
        CullFilter::PicksOnly => Some(1),
        CullFilter::MinRating(_) => Some(2),
    };

    let filter_ctrl = SegmentedControl::new(&["All", "Picks", "★3+"], filter_sel)
        .label("Filmstrip filter")
        .fit()
        .on_pick(|s: &mut AppState, i, tx| {
            let next = match i {
                0 => CullFilter::All,
                1 => CullFilter::PicksOnly,
                _ => CullFilter::MinRating(3),
            };
            s.set_filter_tx(next, tx);
        })
        .build(k);

    let active_tag = s
        .catalog
        .active_entry()
        .map_or(ColorTag::None, |e| e.effective_color_tag());
    let mut tag_picker = row()
        .key("filmstrip-color-tags")
        .role("radiogroup")
        .label("Assign color tag")
        .items_center()
        .gap(space::XXS);
    for &tag in &ColorTag::ALL {
        let selected = active_tag == tag;
        let fill_col = if tag == ColorTag::None {
            k.c.bg_base
        } else {
            color::color_tag_swatch(tag, k)
        };
        tag_picker = tag_picker.child(
            row()
                .key(format!("fs-tag-{}", tag.cli_slug()))
                .role("radio")
                .label(format!("Color tag {}", tag.label()))
                .help(format!("Tag active photo: {}", tag.label()))
                .checked(selected)
                .cursor(Cursor::Pointer)
                .size(size::COLOR_TAG_BTN, size::COLOR_TAG_BTN)
                .center()
                .radius(radius::circle(size::COLOR_TAG_BTN))
                .fill(if selected {
                    k.c.layer_03
                } else {
                    Color::TRANSPARENT
                })
                .border(
                    if selected {
                        border::REGULAR
                    } else {
                        border::NONE
                    },
                    if selected {
                        k.c.border_focus
                    } else {
                        Color::TRANSPARENT
                    },
                )
                .child(
                    el().size(size::COLOR_TAG_DOT, size::COLOR_TAG_DOT)
                        .radius(radius::circle(size::COLOR_TAG_DOT))
                        .fill(fill_col)
                        .border(border::HAIRLINE, k.c.border_strong)
                        .a11y_hidden(),
                )
                .on_press_tx(move |s: &mut AppState, tx| {
                    s.set_active_color_tag_tx(tag, tx);
                }),
        );
    }

    let auto_adv_btn = Button::new("Auto-Adv")
        .variant(if s.auto_advance {
            ButtonVariant::Contrast
        } else {
            ButtonVariant::GhostMuted
        })
        .control_size(ControlSize::Small)
        .help("Advance to next frame automatically after rating, flagging, or tagging")
        .on_press_tx(|s: &mut AppState, tx| {
            s.auto_advance = !s.auto_advance;
            s.sync_window_state(tx);
        })
        .build(k);

    let header = row()
        .key("filmstrip-header")
        .w_full()
        .h(size::FILMSTRIP_HEADER_H)
        .px(space::XL)
        .items_center()
        .justify_between()
        .border_sides(
            [border::NONE, border::NONE, border::REGULAR, border::NONE],
            k.c.border_muted,
        )
        .child(
            row()
                .items_center()
                .gap(space::MD)
                .child(
                    icon(&icons::FOLDER, size::ICON_MD)
                        .color(k.c.icon_muted)
                        .a11y_hidden(),
                )
                .child(
                    text(title_str)
                        .font(ty::control_label_medium(k))
                        .color(k.c.text_base),
                )
                .child(
                    Badge::new(format!("{}/{}", visible.len(), total))
                        .variant(BadgeVariant::Subtle)
                        .build(k),
                )
                .child(
                    Badge::new(format!("{picks} Picks"))
                        .variant(BadgeVariant::Neutral)
                        .build(k),
                ),
        )
        .child(
            row()
                .items_center()
                .gap(space::MD)
                .child(filter_ctrl)
                .child(tag_picker),
        )
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(auto_adv_btn)
                .child(
                    Button::new("Open Folder…")
                        .variant(ButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .icon(&icons::FOLDER_ADD_LEFT)
                        .shortcut("cmd+shift+o")
                        .on_press_tx(|_: &mut AppState, tx| trigger_open_folder(tx))
                        .build(k),
                )
                .child(
                    IconButton::new(&icons::CLOSE)
                        .variant(IconButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .label("Hide Filmstrip")
                        .help("Close filmstrip")
                        .shortcut("F")
                        .on_press_tx(|s: &mut AppState, tx| {
                            s.show_filmstrip = false;
                            s.sync_window_state(tx);
                        })
                        .build(k),
                ),
        );

    let mut strip = row()
        .key("filmstrip-items")
        .items_center()
        .gap(space::LG)
        .px(space::XL)
        .py(space::MD);

    if visible.is_empty() {
        strip = strip.child(
            row()
                .h(size::THUMB_EMPTY_H)
                .px(space::HUD_Y)
                .items_center()
                .child(
                    text("No photos match this filter — press All to show every frame")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                ),
        );
    } else {
        for idx in visible {
            strip = strip.child(thumbnail_card(s, k, idx));
        }
    }

    let scroll_strip = el()
        .key("filmstrip-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll_x()
        .child(strip);

    col()
        .key("filmstrip-bar")
        .role("region")
        .label("Photo filmstrip")
        .w_full()
        .h(size::FILMSTRIP_H)
        .fill(k.c.bg_deep)
        .border_sides(
            [border::REGULAR, border::NONE, border::NONE, border::NONE],
            k.c.border_base,
        )
        .child(header)
        .child(scroll_strip)
}

fn thumbnail_card(s: &AppState, k: &Kit, idx: usize) -> El {
    let entry = &s.catalog.entries[idx];
    let active = s.catalog.active_index == Some(idx);
    let rejected = entry.meta.flag == CullFlag::Reject;
    let picked = entry.meta.flag == CullFlag::Pick;
    let rating = entry.meta.rating;
    let color_tag = entry.effective_color_tag();
    let variant_count = entry.meta.variant_count();

    let name = entry
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("Photo {}", idx + 1));

    let mut preview = el()
        .w(size::THUMB_PREVIEW_W)
        .h(size::THUMB_PREVIEW_H)
        .radius(k.radius.sm)
        .fill(k.c.layer_01)
        .border(border::HAIRLINE, k.c.border_muted);

    if let Some(px) = s.thumbnails.get(&entry.path) {
        preview = preview.pixels_fit(px, SurfaceFit::Fill);
    }

    if color_tag != ColorTag::None {
        preview = preview.child(
            el().pin(Some(space::XS), Some(space::XS), None, None)
                .size(size::COLOR_TAG_DOT, size::COLOR_TAG_DOT)
                .radius(radius::circle(size::COLOR_TAG_DOT))
                .fill(color::color_tag_swatch(color_tag, k))
                .border(border::REGULAR, k.c.bg_deep)
                .a11y_hidden(),
        );
    }

    if variant_count > 1 {
        preview = preview.child(
            row()
                .pin(Some(space::XS), None, None, Some(space::XS))
                .px(space::XS)
                .py(space::MICRO)
                .radius(k.radius.xs)
                .fill(color::hud_surface_strong(k))
                .child(
                    text(format!("V{variant_count}"))
                        .font(ty::mono_micro(k))
                        .color(k.c.text_base),
                ),
        );
    }

    if picked {
        preview = preview.child(
            row()
                .pin(None, Some(space::XS), Some(space::XS), None)
                .size(size::FLAG_BADGE, size::FLAG_BADGE)
                .center()
                .radius(radius::circle(size::FLAG_BADGE))
                .fill(k.c.success.fg)
                .child(
                    icon(&icons::CHECK, size::ICON_XS)
                        .color(k.c.black)
                        .a11y_hidden(),
                ),
        );
    } else if rejected {
        preview = preview.child(
            row()
                .pin(None, Some(space::XS), Some(space::XS), None)
                .size(size::FLAG_BADGE, size::FLAG_BADGE)
                .center()
                .radius(radius::circle(size::FLAG_BADGE))
                .fill(k.c.danger.fg)
                .child(
                    icon(&icons::CLOSE, size::ICON_XS)
                        .color(k.c.white)
                        .a11y_hidden(),
                ),
        );
    }

    let mut stars = row().items_center().gap(space::MICRO);
    for star in 1..=5u8 {
        let filled = star <= rating;
        stars = stars.child(
            el().key(format!("thumb-{idx}-star-{star}"))
                .role("button")
                .label(format!("Rate {name} {star} stars"))
                .cursor(Cursor::Pointer)
                .px(space::MICRO)
                .child(
                    text(if filled { "★" } else { "☆" })
                        .font(ty::caption(k))
                        .color(if filled {
                            k.c.warning.fg
                        } else {
                            k.c.text_faint
                        }),
                )
                .on_press_tx(move |s: &mut AppState, tx| {
                    let next = if rating == star { 0 } else { star };
                    s.set_entry_rating_tx(idx, next, tx);
                }),
        );
    }

    let footer = col()
        .w_full()
        .gap(space::XXS)
        .child(
            text(&name)
                .font(ty::caption(k))
                .color(if active {
                    k.c.text_base
                } else {
                    k.c.text_muted
                })
                .truncate(),
        )
        .child(stars);

    let card = col()
        .key(format!("filmstrip-card-{idx}"))
        .role("button")
        .label(format!("Select photo {name}"))
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .w(size::THUMB_CARD_W)
        .gap(space::XS)
        .padding(space::TIGHT)
        .radius(k.radius.md)
        .fill(if active { k.c.layer_02 } else { k.c.bg_base })
        .hover(k.c.hover)
        .border(
            if active {
                border::ACTIVE
            } else {
                border::HAIRLINE
            },
            color::pill_border(k, active),
        )
        .child(preview)
        .child(footer)
        .on_press_tx(move |s: &mut AppState, tx| {
            s.switch_catalog_index_tx(idx, tx);
        });

    if rejected && !active {
        card.layer_opacity(color::REJECT_CARD_OPACITY)
    } else {
        card
    }
}
