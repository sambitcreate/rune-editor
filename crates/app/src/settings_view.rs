//! Dedicated Settings view for Rune Darkroom (Agent & Local MCP, CLI & Skill, Security).
//!
//! Styled exclusively with `tokens.rs` and Anomaly OpenCode `Kit` primitives.

use rune::{Color, Cursor, El, ScrollSpec, col, el, icon, row, text};
use rune_editor_mcp::{MCP_PROTOCOL_LATEST, McpSettings};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, Divider, Kit, SegmentedControl, Switch,
};
use rune_theme_opencode::icons;

use crate::menu::trigger_watch_hot_folder;
use crate::state::{
    AppState, McpClientGuide, ProofMargin, SettingsSection, ShortcutAction, ShortcutCategory,
};
use crate::tokens::{border, color, radius, size, space, ty};

const PORT_PRESETS: [u16; 3] = [37420, 37421, 38080];

/// Full-workspace Settings view shown when `s.show_settings` is `true`.
pub fn settings_view(s: &AppState, k: &Kit) -> El {
    let sidebar = settings_sidebar(s, k);
    let content = match s.settings_section {
        SettingsSection::AgentMcp => agent_mcp_section(s, k),
        SettingsSection::CliAndSkill => cli_and_skill_section(s, k),
        SettingsSection::Shortcuts => shortcuts_and_workflow_section(s, k),
        SettingsSection::Security => security_section(s, k),
    };

    let scroll_content = col()
        .w_full()
        .px(space::MODAL_PAD)
        .py(space::MODAL_PAD)
        .items_center()
        .child(
            col()
                .w_full()
                .max_w(size::SETTINGS_CONTENT_MAX_W)
                .gap(space::XXL)
                .child(content),
        );

    let scroll_viewport = el()
        .key("settings-scroll")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .scroll(ScrollSpec::default())
        .child(scroll_content);

    row()
        .key("settings-workspace")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(k.c.page)
        .child(sidebar)
        .child(k.scroll_bar(scroll_viewport).grow(1.0).h_full().min_w(0.0))
}

fn settings_sidebar(s: &AppState, k: &Kit) -> El {
    let running = s.is_mcp_running();

    let back_btn = Button::new("Back to Darkroom")
        .variant(ButtonVariant::Neutral)
        .control_size(ControlSize::Regular)
        .icon(&icons::ARROW_LEFT)
        .help("Return to the photo workspace (Esc)")
        .shortcut("Esc")
        .on_press_tx(|s: &mut AppState, tx| s.set_show_settings_tx(false, tx))
        .build(k);

    let nav_item =
        |sec: SettingsSection, label: &'static str, ic: &'static rune::Icon, dot: Option<bool>| {
            let active = s.settings_section == sec;
            let mut r = row()
                .key(format!("settings-nav-{label}"))
                .w_full()
                .px(space::LG)
                .py(space::MD)
                .gap(space::MD)
                .items_center()
                .justify_between()
                .radius(k.radius.md)
                .continuous_corners()
                .fill(if active {
                    k.c.layer_03
                } else {
                    Color::TRANSPARENT
                })
                .border(
                    border::HAIRLINE,
                    if active {
                        k.c.border_base
                    } else {
                        Color::TRANSPARENT
                    },
                )
                .hover(k.c.layer_02)
                .cursor(rune::Cursor::Pointer)
                .on_press(move |s: &mut AppState| {
                    s.settings_section = sec;
                })
                .child(
                    row()
                        .items_center()
                        .gap(space::MD)
                        .child(
                            icon(ic, size::ICON_MD)
                                .color(if active {
                                    k.c.text_base
                                } else {
                                    k.c.text_muted
                                })
                                .a11y_hidden(),
                        )
                        .child(
                            text(label)
                                .font(if active {
                                    ty::control_label_medium(k)
                                } else {
                                    ty::control_label(k)
                                })
                                .color(if active {
                                    k.c.text_base
                                } else {
                                    k.c.text_muted
                                }),
                        ),
                );
            if let Some(is_on) = dot {
                r = r.child(status_dot(k, is_on));
            }
            r
        };

    col()
        .key("settings-sidebar")
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
        .child(Divider::horizontal().build(k))
        .child(
            text("SETTINGS")
                .font(ty::section_header(k))
                .color(k.c.text_muted),
        )
        .child(
            col()
                .w_full()
                .gap(space::XS)
                .child(nav_item(
                    SettingsSection::AgentMcp,
                    "Agent & Local MCP",
                    &icons::MCP,
                    Some(running),
                ))
                .child(nav_item(
                    SettingsSection::CliAndSkill,
                    "CLI & Agent Skill",
                    &icons::TERMINAL,
                    None,
                ))
                .child(nav_item(
                    SettingsSection::Shortcuts,
                    "Shortcuts & Workflow",
                    &icons::GRID_PLUS,
                    if s.hot_folder_watcher.is_some() {
                        Some(true)
                    } else {
                        None
                    },
                ))
                .child(nav_item(
                    SettingsSection::Security,
                    "Security & Storage",
                    &icons::LOCK,
                    None,
                )),
        )
        .child(el().grow(1.0))
        .child(
            col()
                .gap(space::XXS)
                .child(
                    text(format!("MCP Spec {MCP_PROTOCOL_LATEST}"))
                        .font(ty::mono_micro(k))
                        .color(k.c.text_muted),
                )
                .child(
                    text("Localhost 127.0.0.1 only")
                        .font(ty::caption(k))
                        .color(k.c.text_muted),
                ),
        )
}

fn agent_mcp_section(s: &AppState, k: &Kit) -> El {
    let running = s.is_mcp_running();
    let endpoint = s.mcp_settings.endpoint_url();

    // 1. Master MCP Server Card (ON / OFF Switch + Live Status)
    let status_pill = row()
        .px(space::MD)
        .py(space::TINY)
        .gap(space::SM)
        .items_center()
        .radius(k.radius.md)
        .continuous_corners()
        .fill(if running {
            color::mcp_running_bg()
        } else {
            k.c.bg_deep
        })
        .border(
            border::HAIRLINE,
            if running {
                color::mcp_running()
            } else {
                k.c.border_base
            },
        )
        .child(status_dot(k, running))
        .child(
            text(if running {
                format!("Running · 127.0.0.1:{}", s.mcp_settings.port)
            } else {
                "Stopped".to_owned()
            })
            .font(ty::mono_compact(k))
            .color(if running {
                color::mcp_running()
            } else {
                k.c.text_muted
            }),
        );

    let master_switch = Switch::new(running)
        .label("Toggle Local MCP Server")
        .hide_label()
        .on_toggle_tx(|s: &mut AppState, tx| s.toggle_mcp_server_tx(tx))
        .build(k);

    let mut master_card = settings_card(k)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::XXL)
                .child(
                    col()
                        .gap(space::XS)
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            row()
                                .items_center()
                                .gap(space::MD)
                                .child(
                                    icon(&icons::MCP, size::ICON_LG)
                                        .color(k.c.text_base)
                                        .a11y_hidden(),
                                )
                                .child(
                                    text("Local MCP Server")
                                        .font(ty::dialog_title(k))
                                        .color(k.c.text_base),
                                )
                                .child(status_pill),
                        )
                        .child(
                            text("Expose Rune Darkroom over Streamable HTTP on localhost so AI agents can inspect, analyze, preview, and non-destructively edit photos on your behalf.")
                                .font(ty::dialog_body(k))
                                .color(k.c.text_muted)
                                .wrap(),
                        ),
                )
                .child(master_switch),
        );

    if let Some(err) = &s.mcp_error {
        master_card = master_card.child(
            row()
                .w_full()
                .px(space::LG)
                .py(space::MD)
                .radius(k.radius.md)
                .fill(k.c.bg_deep)
                .border(border::HAIRLINE, k.c.danger.fg)
                .child(
                    text(format!("Server error: {err}"))
                        .font(ty::mono_compact(k))
                        .color(k.c.danger.fg),
                ),
        );
    }

    // Live Canvas Edits & Port Controls
    let live_switch = Switch::new(s.mcp_settings.allow_live_canvas_edits)
        .label("Allow agents to edit live open canvas")
        .hide_label()
        .on_toggle_tx(|s: &mut AppState, tx| s.toggle_mcp_live_edits_tx(tx))
        .build(k);

    let mut port_row = row().items_center().gap(space::SM);
    for port in PORT_PRESETS {
        let active = s.mcp_settings.port == port;
        port_row = port_row.child(
            Button::new(format!("{port}"))
                .variant(if active {
                    ButtonVariant::Contrast
                } else {
                    ButtonVariant::Neutral
                })
                .control_size(ControlSize::Small)
                .on_press_tx(move |s: &mut AppState, tx| s.set_mcp_port_tx(port, tx))
                .build(k),
        );
    }

    let endpoint_for_copy = endpoint.clone();
    let url_copied = s.mcp_copied_tag.as_deref() == Some("url");
    let copy_url_btn = Button::new(if url_copied { "Copied" } else { "Copy URL" })
        .variant(ButtonVariant::Neutral)
        .control_size(ControlSize::Small)
        .icon(if url_copied {
            &icons::CIRCLE_CHECK
        } else {
            &icons::COPY
        })
        .on_press_tx(move |s: &mut AppState, tx| {
            tx.copy(endpoint_for_copy.clone());
            s.mcp_copied_tag = Some("url".to_owned());
            s.status_message = Some(format!("Copied MCP endpoint {endpoint_for_copy}"));
        })
        .build(k);

    let activity_label = match &s.mcp_last_activity {
        Some(last) => format!("{} tool call(s) · Last: {last}", s.mcp_request_count),
        None => format!(
            "{} tool call(s) · Protocol {MCP_PROTOCOL_LATEST} · 7 tools · 74 synced params",
            s.mcp_request_count
        ),
    };

    let controls_card = settings_card(k)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::LG)
                .child(
                    col()
                        .gap(space::XXS)
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            text("Allow Live Canvas Edits")
                                .font(ty::control_label_medium(k))
                                .color(k.c.text_base),
                        )
                        .child(
                            text("Agents can apply edits directly to the active darkroom viewport. Every edit pushes to your ⌘Z undo history.")
                                .font(ty::caption(k))
                                .color(k.c.text_muted)
                                .wrap(),
                        ),
                )
                .child(live_switch),
        )
        .child(Divider::horizontal().build(k))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::LG)
                .child(
                    col()
                        .gap(space::XXS)
                        .child(
                            text("Local Endpoint & Port")
                                .font(ty::control_label_medium(k))
                                .color(k.c.text_base),
                        )
                        .child(
                            text(endpoint)
                                .font(ty::mono_value(k))
                                .color(k.c.text_muted),
                        ),
                )
                .child(
                    row()
                        .items_center()
                        .gap(space::MD)
                        .child(port_row)
                        .child(copy_url_btn),
                ),
        )
        .child(Divider::horizontal().build(k))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    text(activity_label)
                        .font(ty::mono_compact(k))
                        .color(k.c.text_muted),
                )
                .child(
                    Badge::new("127.0.0.1 + Origin Guard")
                        .variant(BadgeVariant::Subtle)
                        .build(k),
                ),
        );

    // 2. How to Connect With Agents Card
    let client_idx = match s.mcp_client_guide {
        McpClientGuide::StreamableHttp => 0,
        McpClientGuide::StdioCli => 1,
        McpClientGuide::SkillMd => 2,
    };
    let client_seg = SegmentedControl::new(
        &[
            "Streamable HTTP",
            "Stdio (rd mcp)",
            "Agent Skill (SKILL.md)",
        ],
        Some(client_idx),
    )
    .label("Agent connection method")
    .fit()
    .on_pick(|s: &mut AppState, i, _| {
        s.mcp_client_guide = match i {
            0 => McpClientGuide::StreamableHttp,
            1 => McpClientGuide::StdioCli,
            _ => McpClientGuide::SkillMd,
        };
    })
    .build(k);

    let rd_path = resolve_rd_binary_display();
    let (guide_title, guide_desc, snippet, copy_tag) = match s.mcp_client_guide {
        McpClientGuide::StreamableHttp => (
            "Connect Antigravity, Claude Code, Cursor, or Windsurf (Live GUI)",
            "Turn the Local MCP Server switch ON above, then add this entry to your agent's MCP config. Calls edit the live open canvas in real time with ⌘Z undo.",
            s.mcp_settings.http_client_snippet(),
            "http_cfg",
        ),
        McpClientGuide::StdioCli => (
            "Connect Claude Desktop, Codex, or Headless Agents (Stdio)",
            "Spawns the lightweight `rd mcp` process on demand over stdin/stdout—works even when the Rune Darkroom desktop app is closed.",
            McpSettings::stdio_client_snippet(&rd_path),
            "stdio_cfg",
        ),
        McpClientGuide::SkillMd => (
            "Teach Any Coding Agent via Official Agent Skill",
            "Point Antigravity, Claude Code, OpenCode, or Codex to the repository's `SKILL.md` so agents know the Analyze → Edit → Preview → CIEDE2000 Diff → Export loop.",
            format!(
                "# Claude Code / Antigravity / OpenCode Skill Path\nskills/rune-darkroom/SKILL.md\n\n# Or verify CLI discovery directly:\n{rd_path} schema recipe\n{rd_path} analyze ./photo.ARW --include exif"
            ),
            "skill_cfg",
        ),
    };

    let snippet_for_copy = snippet.clone();
    let is_copied = s.mcp_copied_tag.as_deref() == Some(copy_tag);
    let copy_cfg_btn = Button::new(if is_copied { "Copied!" } else { "Copy Snippet" })
        .variant(ButtonVariant::Contrast)
        .control_size(ControlSize::Small)
        .icon(if is_copied {
            &icons::CIRCLE_CHECK
        } else {
            &icons::COPY
        })
        .on_press_tx(move |s: &mut AppState, tx| {
            tx.copy(snippet_for_copy.clone());
            s.mcp_copied_tag = Some(copy_tag.to_owned());
            s.status_message = Some("Copied MCP agent configuration to clipboard".to_owned());
        })
        .build(k);

    let connect_card = settings_card(k)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    row()
                        .items_center()
                        .gap(space::SM)
                        .child(
                            icon(&icons::LINK, size::ICON_MD)
                                .color(k.c.text_base)
                                .a11y_hidden(),
                        )
                        .child(
                            text("How to Connect AI Agents")
                                .font(ty::dialog_title(k))
                                .color(k.c.text_base),
                        ),
                )
                .child(copy_cfg_btn),
        )
        .child(client_seg)
        .child(
            col()
                .w_full()
                .min_w(0.0)
                .gap(space::XXS)
                .child(
                    text(guide_title)
                        .font(ty::control_label_medium(k))
                        .color(k.c.text_base)
                        .wrap(),
                )
                .child(
                    text(guide_desc)
                        .font(ty::caption(k))
                        .color(k.c.text_muted)
                        .wrap(),
                ),
        )
        .child(code_block(k, &snippet))
        .child(
            col()
                .gap(space::XS)
                .child(
                    text("EXPOSED MCP TOOLS (AUTO-SYNCED WITH GUI REGISTRY)")
                        .font(ty::section_header(k))
                        .color(k.c.text_muted),
                )
                .child(tool_list_summary(k)),
        );

    col()
        .w_full()
        .gap(space::XL)
        .child(master_card)
        .child(controls_card)
        .child(connect_card)
}

fn cli_and_skill_section(s: &AppState, k: &Kit) -> El {
    let rd_path = resolve_rd_binary_display();
    let param_count = rune_editor_core::all_param_descriptors().len();
    let examples = format!(
        "# Discover all {param_count} GUI-synced develop parameters\n{rd_path} schema recipe\n\n# Analyze tone, Lab cast, skin tones & sharpness\n{rd_path} analyze ./shoot --ndjson\n\n# Apply preset blend + targeted nudges non-destructively\n{rd_path} edit photo.ARW --preset classic_chrome --mix 0.8 --set highlights=-0.35\n\n# Verify CIEDE2000 & highlight clipping guardrails\n{rd_path} diff photo.ARW --assert max_highlight_clip_pct=0.5\n\n# Run headless Streamable HTTP MCP from terminal\n{rd_path} mcp --http --port {}",
        s.mcp_settings.port
    );
    let copy_examples = examples.clone();
    let is_copied = s.mcp_copied_tag.as_deref() == Some("cli_ex");

    settings_card(k)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    row()
                        .items_center()
                        .gap(space::SM)
                        .child(
                            icon(&icons::TERMINAL, size::ICON_LG)
                                .color(k.c.text_base)
                                .a11y_hidden(),
                        )
                        .child(
                            text("Headless CLI (`rd`) & Agent Skill")
                                .font(ty::dialog_title(k))
                                .color(k.c.text_base),
                        ),
                )
                .child(
                    Button::new(if is_copied { "Copied!" } else { "Copy Commands" })
                        .variant(ButtonVariant::Neutral)
                        .control_size(ControlSize::Small)
                        .icon(if is_copied {
                            &icons::CIRCLE_CHECK
                        } else {
                            &icons::COPY
                        })
                        .on_press_tx(move |s: &mut AppState, tx| {
                            tx.copy(copy_examples.clone());
                            s.mcp_copied_tag = Some("cli_ex".to_owned());
                            s.status_message = Some("Copied CLI examples to clipboard".to_owned());
                        })
                        .build(k),
                ),
        )
        .child(
            text("Every slider, preset, and geometry control in Rune Darkroom is registered in `crates/core/src/registry.rs`, keeping the GUI, `rd` CLI, and MCP server in strict 1:1 parity.")
                .font(ty::dialog_body(k))
                .color(k.c.text_muted)
                .wrap(),
        )
        .child(code_block(k, &examples))
}

fn shortcuts_and_workflow_section(s: &AppState, k: &Kit) -> El {
    let settings_path_str = McpSettings::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/rune-darkroom/settings.json".to_owned());

    // 1. Hot Folder Auto-Ingest & Proof Margin Card
    let watching = s.hot_folder_watcher.is_some();
    let watch_label = s
        .hot_folder_dir
        .as_ref()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "No hot folder active — click Watch Folder… to auto-ingest".to_owned());

    let mut style_pills = row().w_full().gap(space::XS);
    let none_active = s.hot_folder_style.is_none();
    style_pills = style_pills.child(
        row()
            .key("hot-style-none")
            .role("radio")
            .checked(none_active)
            .cursor(Cursor::Pointer)
            .h(size::PILL_COMPACT_H)
            .px(space::MD)
            .center()
            .radius(k.radius.sm)
            .fill(color::pill_fill(k, none_active))
            .hover(k.c.hover)
            .border(border::HAIRLINE, color::pill_border(k, none_active))
            .child(
                text("None")
                    .font(ty::control_label(k))
                    .color(if none_active {
                        k.c.text_accent
                    } else {
                        k.c.text_muted
                    }),
            )
            .on_press_tx(|s: &mut AppState, tx| s.set_hot_folder_style_tx(None, tx)),
    );
    for preset in s.presets.iter().take(4) {
        let pid = preset.id.to_owned();
        let active = s.hot_folder_style.as_deref() == Some(preset.id);
        style_pills = style_pills.child(
            row()
                .key(format!("hot-style-{}", preset.id))
                .role("radio")
                .checked(active)
                .cursor(Cursor::Pointer)
                .h(size::PILL_COMPACT_H)
                .px(space::MD)
                .center()
                .radius(k.radius.sm)
                .fill(color::pill_fill(k, active))
                .hover(k.c.hover)
                .border(border::HAIRLINE, color::pill_border(k, active))
                .child(
                    text(preset.name)
                        .font(ty::control_label(k))
                        .color(if active {
                            k.c.text_accent
                        } else {
                            k.c.text_muted
                        }),
                )
                .on_press_tx(move |s: &mut AppState, tx| {
                    s.set_hot_folder_style_tx(Some(pid.clone()), tx);
                }),
        );
    }

    let mut margin_pills = row().w_full().gap(space::SM);
    for margin in ProofMargin::ALL {
        let active = s.proof_margin == margin;
        margin_pills = margin_pills.child(
            row()
                .key(format!("settings-margin-{}", margin.slug()))
                .role("radio")
                .checked(active)
                .cursor(Cursor::Pointer)
                .grow(1.0)
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
                    text(format!(
                        "{} ({})",
                        margin.label(),
                        margin.effective_pt_label()
                    ))
                    .font(ty::control_label(k))
                    .color(if active {
                        k.c.text_accent
                    } else {
                        k.c.text_muted
                    }),
                )
                .on_press_tx(move |s: &mut AppState, tx| s.set_proof_margin_tx(margin, tx)),
        );
    }

    let hot_folder_card = col()
        .key("settings-hot-folder-card")
        .w_full()
        .gap(space::MD)
        .padding(space::MODAL_PAD)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(border::HAIRLINE, k.c.border_base)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    row()
                        .items_center()
                        .gap(space::MD)
                        .child(status_dot(k, watching))
                        .child(
                            col()
                                .gap(space::XXS)
                                .child(
                                    text("Hot Folder Auto-Ingest & Proof Margin")
                                        .font(ty::dialog_title(k))
                                        .color(k.c.text_base),
                                )
                                .child(
                                    text(watch_label)
                                        .font(ty::mono_micro(k))
                                        .color(k.c.text_muted)
                                        .truncate(),
                                ),
                        ),
                )
                .child(if watching {
                    row()
                        .gap(space::SM)
                        .child(
                            Button::new("Poll Now")
                                .variant(ButtonVariant::Neutral)
                                .control_size(ControlSize::Small)
                                .on_press_tx(|s: &mut AppState, tx| s.poll_hot_folder_tx(tx))
                                .build(k),
                        )
                        .child(
                            Button::new("Stop")
                                .variant(ButtonVariant::GhostMuted)
                                .control_size(ControlSize::Small)
                                .on_press_tx(|s: &mut AppState, tx| s.stop_hot_folder_watch_tx(tx))
                                .build(k),
                        )
                } else {
                    row().child(
                        Button::new("Watch Folder…")
                            .variant(ButtonVariant::Contrast)
                            .control_size(ControlSize::Small)
                            .on_press_tx(|_: &mut AppState, tx| trigger_watch_hot_folder(tx))
                            .build(k),
                    )
                }),
        )
        .child(
            col()
                .w_full()
                .gap(space::XS)
                .child(
                    text("Auto-Apply Style on Arrival")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                )
                .child(style_pills),
        )
        .child(Divider::horizontal().build(k))
        .child(
            col()
                .w_full()
                .gap(space::XS)
                .child(
                    row()
                        .w_full()
                        .justify_between()
                        .child(
                            text("Proof Margin (⌘⇧M)")
                                .font(ty::control_label(k))
                                .color(k.c.text_muted),
                        )
                        .child(
                            text(s.proof_margin.label())
                                .font(ty::mono_compact(k))
                                .color(k.c.text_base),
                        ),
                )
                .child(margin_pills),
        );

    // 2. Custom Keyboard Shortcuts Card
    let mut shortcut_groups = col().w_full().gap(space::MD);
    for cat in ShortcutCategory::ALL {
        let mut group_col = col().w_full().gap(space::XS).child(
            text(cat.label())
                .font(ty::section_header(k))
                .color(k.c.text_faint),
        );

        if cat == ShortcutCategory::Workflow {
            for (label, chord) in [
                ("Cycle Proof Margin", "⌘⇧M"),
                ("Move Photo + Sidecar to Selects", "⌘J"),
                ("Compare Variant Reference", "⌘⏎"),
                ("Multi-Recipe Export", "⌘⇧E"),
                ("Edit With External Editor (16-bit TIFF)", "⌘⇧W"),
            ] {
                group_col = group_col.child(
                    row()
                        .w_full()
                        .px(space::MD)
                        .py(space::XS)
                        .items_center()
                        .justify_between()
                        .radius(k.radius.sm)
                        .fill(k.c.bg_base)
                        .border(border::HAIRLINE, k.c.border_muted)
                        .child(text(label).font(ty::control_label(k)).color(k.c.text_base))
                        .child(
                            row()
                                .px(space::MD)
                                .py(space::XXS)
                                .radius(k.radius.xs)
                                .fill(k.c.bg_deep)
                                .border(border::HAIRLINE, k.c.border_base)
                                .child(text(chord).font(ty::mono_compact(k)).color(k.c.text_muted)),
                        ),
                );
            }
        } else {
            for action in ShortcutAction::ALL
                .into_iter()
                .filter(|a| a.category() == cat)
            {
                let recording = s.recording_shortcut == Some(action);
                let customized = s.shortcuts.is_customized(action);
                let key_label = if recording {
                    "Press key…".to_owned()
                } else {
                    s.shortcuts.display_key(action)
                };
                let pill_border = if recording {
                    k.c.border_focus
                } else if customized {
                    color::film_negative_accent()
                } else {
                    k.c.border_base
                };
                let key_color = if recording {
                    k.c.text_accent
                } else if customized {
                    color::film_negative_accent()
                } else {
                    k.c.text_base
                };

                let mut left_row = row().items_center().gap(space::SM).child(
                    text(action.label())
                        .font(ty::control_label(k))
                        .color(k.c.text_base),
                );
                if customized {
                    left_row = left_row.child(
                        text("Custom")
                            .font(ty::mono_micro(k))
                            .color(color::film_negative_accent()),
                    );
                }

                group_col = group_col.child(
                    row()
                        .key(format!("shortcut-row-{}", action.id()))
                        .w_full()
                        .px(space::MD)
                        .py(space::XS)
                        .items_center()
                        .justify_between()
                        .radius(k.radius.sm)
                        .fill(k.c.bg_base)
                        .border(border::HAIRLINE, k.c.border_muted)
                        .child(left_row)
                        .child(
                            row()
                                .key(format!("shortcut-btn-{}", action.id()))
                                .role("button")
                                .label(format!("Remap {}", action.label()))
                                .help("Click then press a key to customize binding")
                                .cursor(Cursor::Pointer)
                                .px(space::MD)
                                .py(space::XXS)
                                .radius(k.radius.xs)
                                .fill(if recording { k.c.layer_03 } else { k.c.bg_deep })
                                .hover(k.c.hover)
                                .border(border::REGULAR, pill_border)
                                .child(text(key_label).font(ty::mono_compact(k)).color(key_color))
                                .on_press(move |s: &mut AppState| {
                                    if s.recording_shortcut == Some(action) {
                                        s.recording_shortcut = None;
                                    } else {
                                        s.recording_shortcut = Some(action);
                                        s.status_message = Some(format!(
                                            "Press any key to rebind '{}' (Esc to cancel)",
                                            action.label()
                                        ));
                                    }
                                }),
                        ),
                );
            }
        }
        shortcut_groups = shortcut_groups.child(group_col);
    }

    let shortcuts_card = col()
        .key("settings-shortcuts-card")
        .w_full()
        .gap(space::MD)
        .padding(space::MODAL_PAD)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .border(border::HAIRLINE, k.c.border_base)
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    col()
                        .gap(space::XXS)
                        .child(
                            text("Keyboard Shortcuts")
                                .font(ty::dialog_title(k))
                                .color(k.c.text_base),
                        )
                        .child(
                            text(format!(
                                "Click any keycap to rebind • Saved to {settings_path_str}"
                            ))
                            .font(ty::caption(k))
                            .color(k.c.text_muted),
                        ),
                )
                .child(
                    Button::new("Reset Defaults")
                        .variant(ButtonVariant::Neutral)
                        .control_size(ControlSize::Small)
                        .disabled(!s.shortcuts.is_any_customized())
                        .on_press_tx(|s: &mut AppState, tx| s.reset_shortcut_defaults_tx(tx))
                        .build(k),
                ),
        )
        .child(shortcut_groups);

    col()
        .w_full()
        .gap(space::XXL)
        .child(hot_folder_card)
        .child(shortcuts_card)
}

fn security_section(_s: &AppState, k: &Kit) -> El {
    let config_path = McpSettings::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/rune-darkroom/settings.json".to_owned());

    let bullet = |title: &'static str, body: &'static str| {
        col()
            .w_full()
            .min_w(0.0)
            .gap(space::XXS)
            .child(
                text(title)
                    .font(ty::control_label_medium(k))
                    .color(k.c.text_base),
            )
            .child(text(body).font(ty::caption(k)).color(k.c.text_muted).wrap())
    };

    settings_card(k)
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    icon(&icons::LOCK, size::ICON_LG)
                        .color(k.c.text_base)
                        .a11y_hidden(),
                )
                .child(
                    text("Local MCP Security & Nondestructive Guarantees")
                        .font(ty::dialog_title(k))
                        .color(k.c.text_base),
                ),
        )
        .child(Divider::horizontal().build(k))
        .child(bullet(
            "Loopback-Only Binding (127.0.0.1)",
            "The embedded Streamable HTTP server binds exclusively to 127.0.0.1 and is never exposed to Wi-Fi, LAN, or external interfaces.",
        ))
        .child(bullet(
            "MCP 2025-11-25 Origin Validation (DNS Rebinding Protection)",
            "Every HTTP request with an untrusted `Origin` header is rejected immediately with `403 Forbidden`. Only localhost origins and native non-browser agents are permitted.",
        ))
        .child(bullet(
            "Inviolable Originals & ⌘Z Undo",
            "Agents never overwrite source RAW/raster files. Sidecars use atomic write-to-temp + rename with provenance history, and live canvas edits push onto your ⌘Z undo stack.",
        ))
        .child(Divider::horizontal().build(k))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    text(format!("Settings file: {config_path}"))
                        .font(ty::mono_compact(k))
                        .color(k.c.text_muted),
                ),
        )
}

fn settings_card(k: &Kit) -> El {
    col()
        .w_full()
        .px(space::XXL)
        .py(space::XXL)
        .gap(space::LG)
        .radius(k.radius.lg)
        .continuous_corners()
        .fill(k.c.bg_base)
        .border(border::REGULAR, k.c.border_base)
}

fn code_block(k: &Kit, code: &str) -> El {
    col()
        .w_full()
        .px(space::XL)
        .py(space::XL)
        .radius(k.radius.md)
        .continuous_corners()
        .fill(color::code_surface(k))
        .border(border::HAIRLINE, k.c.border_base)
        .child(text(code).font(ty::mono_value(k)).color(k.c.text_base))
}

fn tool_list_summary(k: &Kit) -> El {
    let param_count = rune_editor_core::all_param_descriptors().len();
    let schema_desc =
        format!("Read-only · {param_count} parameters, bounds, presets, active canvas");
    let items = [
        ("rune_get_schema", schema_desc.as_str()),
        (
            "rune_inspect_photo",
            "Read-only · EXIF metadata, rating, flag, recipe overrides",
        ),
        (
            "rune_analyze_photo",
            "Read-only · DR, clipping %, Lab cast, skin guard, focus",
        ),
        (
            "rune_edit_photo",
            "Non-destructive · Preset blend, patch, auto WB/tone, ⌘Z undo",
        ),
        (
            "rune_preview_photo",
            "Read-only · Inline base64 JPEG preview + clipping telemetry",
        ),
        (
            "rune_diff_photo",
            "Read-only · Exact CIEDE2000 perceptual & clip delta",
        ),
        (
            "rune_cull_or_export",
            "Safe · Stars, flags, 7 color tags, watermark & multi-recipe export",
        ),
        (
            "rune_manage_workflow",
            "Non-destructive · Virtual variants, user/stacked styles, selective clipboard",
        ),
        (
            "rune_edit_layers",
            "Non-destructive · Adjustment layers, composable masks, AI subject/bg, heal/clone",
        ),
    ];
    let mut list = col().w_full().gap(space::XS);
    for (name, desc) in items {
        list = list.child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::MD)
                .child(text(name).font(ty::mono_compact(k)).color(k.c.text_base))
                .child(
                    text(desc)
                        .font(ty::caption(k))
                        .color(k.c.text_muted)
                        .truncate(),
                ),
        );
    }
    list
}

fn status_dot(k: &Kit, running: bool) -> El {
    el().w(size::STATUS_DOT)
        .h(size::STATUS_DOT)
        .radius(radius::circle(size::STATUS_DOT))
        .fill(if running {
            color::mcp_running()
        } else {
            color::mcp_stopped(k)
        })
}

fn resolve_rd_binary_display() -> String {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("rd");
        if sibling.exists() {
            return sibling.display().to_string();
        }
    }
    "rd".to_owned()
}
