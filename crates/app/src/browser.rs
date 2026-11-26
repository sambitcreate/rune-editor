//! Capture One-style Multi-Column Grid Browser (`G`) with faceted culling filters
//! (`All | Picks | ★3+`, 7 Color Tags, case-insensitive search), Auto-Advance toggle,
//! Variant badges (`V2`), Color Tag dots, and interactive star ratings.

use rune::{Cursor, El, ScrollSpec, SurfaceFit, col, el, icon, row, text};
use rune_editor_core::{ColorTag, CullFilter, CullFlag};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Switch,
    TextInput,
};
use rune_theme_opencode::icons;

use crate::menu::{trigger_open_folder, trigger_watch_hot_folder};
use crate::state::AppState;
use crate::tokens::{border, color, radius, size, space, ty};

pub fn grid_browser_view(s: &AppState, k: &Kit) -> El {
    let (total, picks, _rejects) = s.catalog.counts();
    let visible = s.catalog.filtered_indices();

    let title_str = s
        .catalog
        .folder
        .as_ref()
        .and_then(|f| f.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Session Browser".to_owned());

    let filter_sel = match s.catalog.filter {
        CullFilter::All => Some(0),
        CullFilter::PicksOnly => Some(1),
        CullFilter::MinRating(_) => Some(2),
    };
    let filter_ctrl = SegmentedControl::new(&["All", "Picks", "★3+"], filter_sel)
        .label("Grid browser filter")
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

    // 7 Capture One Color Tag filter dots (+ Clear dot)
    let mut tag_filter_row = row()
        .key("grid-tag-filters")
        .role("radiogroup")
        .label("Filter by color tag")
        .items_center()
        .gap(space::XS);
    for &tag in &ColorTag::ALL {
        let active = match tag {
            ColorTag::None => s.catalog.color_tag_filter.is_none(),
            t => s.catalog.color_tag_filter == Some(t),
        };
        let fill_col = if tag == ColorTag::None {
            k.c.bg_base
        } else {
            color::color_tag_swatch(tag, k)
        };
        let help = if tag == ColorTag::None {
            "Show all color tags".to_owned()
        } else {
            format!("Filter by {} tag", tag.label())
        };
        let dot_btn = row()
            .key(format!("grid-filter-tag-{}", tag.cli_slug()))
            .role("radio")
            .label(help.clone())
            .help(help)
            .checked(active)
            .focusable(true)
            .focus_ring(
                border::FOCUS_RING,
                border::FOCUS_OFFSET_TIGHT,
                k.c.border_focus,
            )
            .cursor(Cursor::Pointer)
            .size(size::COLOR_TAG_BTN, size::COLOR_TAG_BTN)
            .center()
            .radius(radius::circle(size::COLOR_TAG_BTN))
            .fill(if active {
                k.c.layer_03
            } else {
                Color::TRANSPARENT
            })
            .border(
                if active {
                    border::REGULAR
                } else {
                    border::NONE
                },
                if active {
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
                let next = if tag == ColorTag::None || s.catalog.color_tag_filter == Some(tag) {
                    None
                } else {
                    Some(tag)
                };
                s.set_color_tag_filter_tx(next, tx);
            });
        tag_filter_row = tag_filter_row.child(dot_btn);
    }

    let search_input = TextInput::new(&s.browser_search_edit)
        .placeholder("Search file, camera, lens…")
        .width(Some(220.0))
        .on_edit(|s: &mut AppState| &mut s.browser_search_edit)
        .on_change(|s: &mut AppState, tx| {
            let q = s.browser_search_edit.text.clone();
            s.set_browser_search_tx(q, tx);
        })
        .build(k);

    let auto_adv_row = row()
        .items_center()
        .gap(space::SM)
        .child(
            text("Auto-Advance")
                .font(ty::caption(k))
                .color(if s.auto_advance {
                    k.c.text_base
                } else {
                    k.c.text_muted
                }),
        )
        .child(
            Switch::new(s.auto_advance)
                .label("Auto-Advance on rate, flag, or tag")
                .hide_label()
                .on_toggle_tx(|s: &mut AppState, tx| {
                    s.auto_advance = !s.auto_advance;
                    s.sync_window_state(tx);
                })
                .build(k),
        );

    let hot_watching = s.hot_folder_watcher.is_some();
    let hot_btn = if hot_watching {
        row()
            .items_center()
            .gap(space::XS)
            .child(
                Button::new("Hot Folder ●")
                    .variant(ButtonVariant::Neutral)
                    .control_size(ControlSize::Small)
                    .help("Stop watching hot folder for new arrivals")
                    .on_press_tx(|s: &mut AppState, tx| s.stop_hot_folder_watch_tx(tx))
                    .build(k),
            )
            .child(
                Button::new("Poll")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .help("Check hot folder for newly arrived photos")
                    .on_press_tx(|s: &mut AppState, tx| s.poll_hot_folder_tx(tx))
                    .build(k),
            )
    } else {
        row().child(
            Button::new("Hot Folder")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .help("Watch active folder (or pick a folder) for newly arrived photos")
                .on_press_tx(|s: &mut AppState, tx| {
                    if let Some(dir) = s.catalog.folder.clone() {
                        s.start_hot_folder_watch_tx(dir, tx);
                    } else {
                        trigger_watch_hot_folder(tx);
                    }
                })
                .build(k),
        )
    };

    let header = col()
        .key("grid-browser-header")
        .w_full()
        .gap(space::MD)
        .px(space::HUD_X)
        .py(space::LG)
        .fill(k.c.bg_deep)
        .border_sides(
            [border::NONE, border::NONE, border::REGULAR, border::NONE],
            k.c.border_base,
        )
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::LG)
                .child(
                    row()
                        .items_center()
                        .gap(space::MD)
                        .child(
                            icon(&icons::GRID_PLUS, size::ICON_MD)
                                .color(k.c.text_base)
                                .a11y_hidden(),
                        )
                        .child(
                            text(title_str)
                                .font(ty::brand_title(k))
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
                        .child(auto_adv_row)
                        .child(hot_btn)
                        .child(
                            Button::new("Open Folder…")
                                .variant(ButtonVariant::GhostMuted)
                                .control_size(ControlSize::Small)
                                .icon(&icons::FOLDER_ADD_LEFT)
                                .on_press_tx(|_: &mut AppState, tx| trigger_open_folder(tx))
                                .build(k),
                        )
                        .child(
                            Button::new("Viewer")
                                .variant(ButtonVariant::Contrast)
                                .control_size(ControlSize::Small)
                                .icon(&icons::PHOTO)
                                .shortcut("G")
                                .on_press_tx(|s: &mut AppState, tx| s.toggle_grid_browser_tx(tx))
                                .build(k),
                        ),
                ),
        )
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::LG)
                .child(
                    row()
                        .items_center()
                        .gap(space::LG)
                        .child(filter_ctrl)
                        .child(tag_filter_row),
                )
                .child(search_input),
        );

    let mut grid_rows = col()
        .key("grid-browser-cards")
        .w_full()
        .gap(space::XXL)
        .padding(space::HUD_X);

    if visible.is_empty() {
        grid_rows = grid_rows.child(
            row().w_full().h(180.0).center().child(
                text("No photographs match the current filter or search query")
                    .font(ty::dialog_body(k))
                    .color(k.c.text_muted),
            ),
        );
    } else {
        for chunk in visible.chunks(4) {
            let mut r = row().w_full().gap(space::XXL);
            for &idx in chunk {
                r = r.child(grid_photo_card(s, k, idx));
            }
            grid_rows = grid_rows.child(r);
        }
    }

    let scroll_vp = col()
        .key("grid-browser-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .child(grid_rows);

    col()
        .key("grid-browser-view")
        .role("region")
        .label("Grid browser")
        .grow(1.0)
        .w_full()
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(k.c.page)
        .child(header)
        .child(k.scroll_bar(scroll_vp))
}

use rune::Color;

fn grid_photo_card(s: &AppState, k: &Kit, idx: usize) -> El {
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

    let meta_line = match (entry.camera_label.as_deref(), entry.lens_label.as_deref()) {
        (Some(c), Some(l)) => format!("{c} · {l}"),
        (Some(c), None) => c.to_owned(),
        (None, Some(l)) => l.to_owned(),
        (None, None) => entry
            .meta
            .recipe
            .preset_name
            .clone()
            .unwrap_or_else(|| "16-bit Linear".to_owned()),
    };

    let mut preview = el()
        .w_full()
        .h(size::GRID_PREVIEW_H)
        .radius(k.radius.sm)
        .fill(k.c.layer_01)
        .border(border::HAIRLINE, k.c.border_muted);

    if let Some(px) = s.thumbnails.get(&entry.path) {
        preview = preview.pixels_fit(px, SurfaceFit::Fill);
    }

    if color_tag != ColorTag::None {
        preview = preview.child(
            el().pin(Some(space::SM), Some(space::SM), None, None)
                .size(size::COLOR_TAG_DOT + 2.0, size::COLOR_TAG_DOT + 2.0)
                .radius(radius::circle(size::COLOR_TAG_DOT + 2.0))
                .fill(color::color_tag_swatch(color_tag, k))
                .border(border::REGULAR, k.c.bg_deep)
                .a11y_hidden(),
        );
    }

    if variant_count > 1 {
        preview = preview.child(
            row()
                .pin(Some(space::SM), None, None, Some(space::SM))
                .px(space::TIGHT)
                .py(space::MICRO)
                .radius(k.radius.xs)
                .fill(color::hud_surface_strong(k))
                .border(border::HAIRLINE, k.c.border_strong)
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
                .pin(None, Some(space::SM), Some(space::SM), None)
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
                .pin(None, Some(space::SM), Some(space::SM), None)
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
            el().key(format!("grid-{idx}-star-{star}"))
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

    let tag_badge = if color_tag != ColorTag::None {
        text(color_tag.label())
            .font(ty::mono_micro(k))
            .color(k.c.text_faint)
    } else {
        text("").font(ty::mono_micro(k)).color(k.c.text_faint)
    };

    let card = col()
        .key(format!("grid-card-{idx}"))
        .role("button")
        .label(format!("Select photo {name}"))
        .focusable(true)
        .focus_ring(
            border::FOCUS_RING,
            border::FOCUS_OFFSET_TIGHT,
            k.c.border_focus,
        )
        .cursor(Cursor::Pointer)
        .w(size::GRID_CARD_W)
        .gap(space::SM)
        .padding(space::MD)
        .radius(k.radius.lg)
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
        .child(
            text(&name)
                .font(ty::control_label_medium(k))
                .color(if active {
                    k.c.text_base
                } else {
                    k.c.text_muted
                })
                .truncate(),
        )
        .child(
            text(meta_line)
                .font(ty::mono_micro(k))
                .color(k.c.text_faint)
                .truncate(),
        )
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(stars)
                .child(tag_badge),
        )
        .on_press_tx(move |s: &mut AppState, tx| {
            s.switch_catalog_index_tx(idx, tx);
        });

    if rejected && !active {
        card.layer_opacity(color::REJECT_CARD_OPACITY)
    } else {
        card
    }
}
