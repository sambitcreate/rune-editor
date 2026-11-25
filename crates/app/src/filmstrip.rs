//! Collapsible bottom Filmstrip & Folder Culling bar (`F` or toolbar toggle).
//!
//! Renders high-speed `rune::Pixels` thumbnail cards with star ratings (`1`–`5`),
//! Pick (`P`) / Reject (`X`) badges, filter pills (`All | Picks | ★+`), and instant
//! keyboard (`←`/`→`) or click switching.

use rune::{Cursor, El, SurfaceFit, col, el, icon, row, text};
use rune_editor_core::{CullFilter, CullFlag};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit,
    SegmentedControl,
};
use rune_theme_opencode::icons;

use crate::menu::trigger_open_folder;
use crate::state::AppState;

pub const FILMSTRIP_HEIGHT: f32 = 160.0;

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

    let header = row()
        .key("filmstrip-header")
        .w_full()
        .h(34.0)
        .px(12.0)
        .items_center()
        .justify_between()
        .border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_muted)
        .child(
            row()
                .items_center()
                .gap(8.0)
                .child(
                    icon(&icons::FOLDER, 13.0)
                        .color(k.c.icon_muted)
                        .a11y_hidden(),
                )
                .child(
                    text(title_str)
                        .font(k.ty.text_xsmall_medium)
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
        .child(filter_ctrl)
        .child(
            row()
                .items_center()
                .gap(6.0)
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
        .gap(10.0)
        .px(12.0)
        .py(8.0);

    if visible.is_empty() {
        strip = strip.child(
            row().h(92.0).px(16.0).items_center().child(
                text("No photos match this filter — press All to show every frame")
                    .font(k.ty.text_xsmall)
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
        .h(FILMSTRIP_HEIGHT)
        .fill(k.c.bg_deep)
        .border_sides([1.0, 0.0, 0.0, 0.0], k.c.border_base)
        .child(header)
        .child(scroll_strip)
}

fn thumbnail_card(s: &AppState, k: &Kit, idx: usize) -> El {
    let entry = &s.catalog.entries[idx];
    let active = s.catalog.active_index == Some(idx);
    let rejected = entry.meta.flag == CullFlag::Reject;
    let picked = entry.meta.flag == CullFlag::Pick;
    let rating = entry.meta.rating;

    let name = entry
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("Photo {}", idx + 1));

    let mut preview = el()
        .w(140.0)
        .h(74.0)
        .radius(k.radius.sm)
        .fill(k.c.layer_01)
        .border(0.5, k.c.border_muted);

    if let Some(px) = s.thumbnails.get(&entry.path) {
        preview = preview.pixels_fit(px, SurfaceFit::Fill);
    }

    // Top-right Pick / Reject badge overlay
    if picked {
        preview = preview.child(
            row()
                .pin(None, Some(4.0), Some(4.0), None)
                .size(16.0, 16.0)
                .center()
                .radius(8.0)
                .fill(k.c.success.fg)
                .child(icon(&icons::CHECK, 10.0).color(k.c.black).a11y_hidden()),
        );
    } else if rejected {
        preview = preview.child(
            row()
                .pin(None, Some(4.0), Some(4.0), None)
                .size(16.0, 16.0)
                .center()
                .radius(8.0)
                .fill(k.c.danger.fg)
                .child(icon(&icons::CLOSE, 10.0).color(k.c.white).a11y_hidden()),
        );
    }

    // Compact interactive 5-star rating row
    let mut stars = row().items_center().gap(1.0);
    for star in 1..=5u8 {
        let filled = star <= rating;
        stars = stars.child(
            el().key(format!("thumb-{idx}-star-{star}"))
                .role("button")
                .label(format!("Rate {name} {star} stars"))
                .cursor(Cursor::Pointer)
                .px(1.0)
                .child(
                    text(if filled { "★" } else { "☆" })
                        .font(k.ty.small)
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
        .gap(2.0)
        .child(
            text(&name)
                .font(k.ty.small)
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
        .focus_ring(2.0, 1.0, k.c.border_focus)
        .cursor(Cursor::Pointer)
        .w(150.0)
        .gap(4.0)
        .padding(5.0)
        .radius(k.radius.md)
        .fill(if active { k.c.layer_02 } else { k.c.bg_base })
        .hover(k.c.hover)
        .border(
            if active { 1.5 } else { 0.5 },
            if active {
                k.c.border_focus
            } else {
                k.c.border_base
            },
        )
        .child(preview)
        .child(footer)
        .on_press_tx(move |s: &mut AppState, tx| {
            s.switch_catalog_index_tx(idx, tx);
        });

    if rejected && !active {
        card.layer_opacity(0.55)
    } else {
        card
    }
}
