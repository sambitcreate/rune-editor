//! Dedicated Settings view for Rune Darkroom (`Agent & Local MCP`, `CLI & Agent Skill`,
//! `Shortcuts & Workflow`, `Security & Storage`), inheriting the exact locked `Studio Airy`
//! macOS System Settings aesthetic: grouped `layer_01` cards (`CARD_PAD = 14pt`,
//! `CARD_GAP = 12pt`, `ROW_GAP = 8pt`), crisp hairline separators, trailing `ⓘ` hover info
//! badges replacing walls of helper prose, and inline `▸` progressive disclosure drawers.

use rune::{Color, Cursor, El, ScrollSpec, col, el, icon, row, text};
use rune_editor_mcp::{MCP_PROTOCOL_LATEST, McpSettings};
use rune_kit::{
    Badge, BadgeVariant, Button, ButtonVariant, ControlSize, Divider, Kit, SegmentedControl, Switch,
};
use rune_theme_opencode::icons;

use crate::menu::trigger_watch_hot_folder;
use crate::state::{
    AppState, InfoTip, McpClientGuide, ProofMargin, SettingsSection, ShortcutAction,
    ShortcutCategory,
};
use crate::tokens::{border, color, radius, size, space, ty};
use crate::widgets::{advanced_disclosure_row, info_badge};

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
                .gap(space::CARD_GAP)
                .child(content),
        );

    let scroll_viewport = col()
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
        .child(k.scroll_bar(scroll_viewport))
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
                .py(space::ROW_GAP)
                .gap(space::ROW_GAP)
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
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut AppState| {
                    s.settings_section = sec;
                    s.hovered_info = None;
                })
                .child(
                    row()
                        .items_center()
                        .gap(space::ROW_GAP)
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
        .px(space::CARD_PAD)
        .py(space::CARD_PAD)
        .gap(space::CARD_GAP)
        .fill(k.c.bg_deep)
        .border_sides(
            [border::NONE, border::REGULAR, border::NONE, border::NONE],
            k.c.border_base,
        )
        .child(back_btn)
        .child(row_hairline(k))
        .child(
            text("SETTINGS")
                .font(ty::section_header(k))
                .color(k.c.text_faint),
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
                        .color(k.c.text_faint),
                ),
        )
}

fn agent_mcp_section(s: &AppState, k: &Kit) -> El {
    let running = s.is_mcp_running();
    let endpoint = s.mcp_settings.endpoint_url();
    let param_count = rune_editor_core::all_param_descriptors().len();

    let status_pill = row()
        .px(space::MD)
        .py(space::XXS)
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

    let mcp_tip = InfoTip {
        title: "Local MCP Server (2025-11-25)",
        shortcut: Some("⌘,"),
        description: "Exposes Rune Darkroom over Streamable HTTP on 127.0.0.1 so AI agents can inspect, analyze, preview, and non-destructively edit photos on your behalf.",
        cli_key: "rd mcp --http --port 37420  ·  127.0.0.1 loopback only",
        tier: "Settings · MCP",
    };

    let master_switch = Switch::new(running)
        .label("Toggle Local MCP Server")
        .hide_label()
        .on_toggle_tx(|s: &mut AppState, tx| s.toggle_mcp_server_tx(tx))
        .build(k);

    let mut master_card = settings_card(k, "settings-mcp-master").child(
        row()
            .w_full()
            .items_center()
            .justify_between()
            .gap(space::GROUP_GAP)
            .child(
                row()
                    .items_center()
                    .gap(space::ROW_GAP)
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
                    .child(info_badge(
                        k,
                        mcp_tip,
                        s.hovered_info.is_some_and(|h| h.title == mcp_tip.title),
                    ))
                    .child(status_pill),
            )
            .child(master_switch),
    );

    if let Some(err) = &s.mcp_error {
        master_card = master_card.child(
            row()
                .w_full()
                .px(space::LG)
                .py(space::ROW_GAP)
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

    // 2. Grouped macOS System Settings Controls Card
    let live_tip = InfoTip {
        title: "Allow Live Canvas Edits",
        shortcut: Some("⌘Z"),
        description: "Allows connected AI agents to update the live open darkroom viewport in real time. Every agent edit pushes cleanly onto your ⌘Z undo stack.",
        cli_key: "settings.json → allow_live_canvas_edits",
        tier: "Settings · Safety",
    };
    let live_switch = Switch::new(s.mcp_settings.allow_live_canvas_edits)
        .label("Allow agents to edit live open canvas")
        .hide_label()
        .on_toggle_tx(|s: &mut AppState, tx| s.toggle_mcp_live_edits_tx(tx))
        .build(k);

    let mut port_row = row().items_center().gap(space::PILL_GAP);
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

    let endpoint_tip = InfoTip {
        title: "Local Endpoint & Port",
        shortcut: None,
        description: "Bound exclusively to 127.0.0.1 loopback with strict RFC 6454 Origin & Host header validation.",
        cli_key: "http://127.0.0.1:<port>/mcp  ·  settings.json → port",
        tier: "Settings · Network",
    };

    let activity_label = match &s.mcp_last_activity {
        Some(last) => format!("{} tool call(s) · Last: {last}", s.mcp_request_count),
        None => format!(
            "{} tool call(s) · {MCP_PROTOCOL_LATEST} · 9 tools · {param_count} synced params",
            s.mcp_request_count
        ),
    };

    let controls_card = settings_card(k, "settings-mcp-controls")
        .child(settings_row(
            k,
            "Allow Live Canvas Edits",
            live_tip,
            s.hovered_info,
            live_switch,
        ))
        .child(row_hairline(k))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::CARD_GAP)
                .child(
                    col()
                        .gap(space::XXS)
                        .child(
                            row()
                                .items_center()
                                .gap(space::SM)
                                .child(
                                    text("Local Endpoint & Port")
                                        .font(ty::control_label_medium(k))
                                        .color(k.c.text_base),
                                )
                                .child(info_badge(
                                    k,
                                    endpoint_tip,
                                    s.hovered_info
                                        .is_some_and(|h| h.title == endpoint_tip.title),
                                )),
                        )
                        .child(
                            text(endpoint)
                                .font(ty::mono_compact(k))
                                .color(k.c.text_muted),
                        ),
                )
                .child(
                    row()
                        .items_center()
                        .gap(space::ROW_GAP)
                        .child(port_row)
                        .child(copy_url_btn),
                ),
        )
        .child(row_hairline(k))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::ROW_GAP)
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

    // 3. Connect AI Agents Card with Studio Airy inline progressive disclosure
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
    let (guide_title, snippet, copy_tag) = match s.mcp_client_guide {
        McpClientGuide::StreamableHttp => (
            "Antigravity, Claude Code, Cursor & Windsurf (Live GUI)",
            s.mcp_settings.http_client_snippet(),
            "http_cfg",
        ),
        McpClientGuide::StdioCli => (
            "Claude Desktop, Codex & Headless Agents (Stdio)",
            McpSettings::stdio_client_snippet(&rd_path),
            "stdio_cfg",
        ),
        McpClientGuide::SkillMd => (
            "Official Agent Skill Path (SKILL.md)",
            format!(
                "# Agent Skill Path\nskills/rune-darkroom/SKILL.md\n\n# Verify CLI schema discovery:\n{rd_path} schema recipe"
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

    let connect_tip = InfoTip {
        title: "Connect AI Agents",
        shortcut: None,
        description: "Choose Streamable HTTP for live GUI editing with ⌘Z undo, Stdio (`rd mcp`) for headless automation, or point agents to `skills/rune-darkroom/SKILL.md`.",
        cli_key: "skills/rune-darkroom/SKILL.md  ·  rd mcp",
        tier: "Settings · Agents",
    };

    let mut connect_card = settings_card(k, "settings-mcp-connect")
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    row()
                        .items_center()
                        .gap(space::ROW_GAP)
                        .child(
                            icon(&icons::LINK, size::ICON_MD)
                                .color(k.c.text_base)
                                .a11y_hidden(),
                        )
                        .child(
                            text("Agent Integration & Tools")
                                .font(ty::dialog_title(k))
                                .color(k.c.text_base),
                        )
                        .child(info_badge(
                            k,
                            connect_tip,
                            s.hovered_info.is_some_and(|h| h.title == connect_tip.title),
                        )),
                )
                .child(copy_cfg_btn),
        )
        .child(advanced_disclosure_row(
            k,
            "settings-disc-snippets",
            "Connection Config Snippets",
            s.settings_mcp_snippets_open,
            0,
            3,
            |s| s.settings_mcp_snippets_open = !s.settings_mcp_snippets_open,
        ));

    if s.settings_mcp_snippets_open {
        connect_card = connect_card
            .child(client_seg)
            .child(
                text(guide_title)
                    .font(ty::control_label(k))
                    .color(k.c.text_muted),
            )
            .child(code_block(k, &snippet));
    }

    connect_card = connect_card.child(advanced_disclosure_row(
        k,
        "settings-disc-tools",
        "Exposed MCP Tools (9)",
        s.settings_mcp_tools_open,
        0,
        9,
        |s| s.settings_mcp_tools_open = !s.settings_mcp_tools_open,
    ));

    if s.settings_mcp_tools_open {
        connect_card = connect_card.child(tool_list_summary(k));
    }

    col()
        .w_full()
        .gap(space::CARD_GAP)
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

    let cli_tip = InfoTip {
        title: "1:1 GUI ↔ CLI ↔ MCP Parity",
        shortcut: None,
        description: "Every slider, preset, geometry option, and layer parameter is registered in `crates/core/src/registry.rs`, keeping GUI, `rd` CLI, and MCP strictly synchronized.",
        cli_key: "rd schema recipe  ·  rd cli search <query>",
        tier: "Settings · CLI",
    };

    let skill_tip = InfoTip {
        title: "Agent Skill Discovery",
        shortcut: None,
        description: "Point Antigravity, Claude Code, OpenCode, or Codex to `skills/rune-darkroom/SKILL.md` for the full Analyze → Edit → Preview → CIEDE2000 Diff → Export loop.",
        cli_key: "skills/rune-darkroom/SKILL.md",
        tier: "Settings · Skill",
    };

    let cli_card = settings_card(k, "settings-cli-card")
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    row()
                        .items_center()
                        .gap(space::ROW_GAP)
                        .child(
                            icon(&icons::TERMINAL, size::ICON_LG)
                                .color(k.c.text_base)
                                .a11y_hidden(),
                        )
                        .child(
                            text("Headless CLI (`rd`)")
                                .font(ty::dialog_title(k))
                                .color(k.c.text_base),
                        )
                        .child(info_badge(
                            k,
                            cli_tip,
                            s.hovered_info.is_some_and(|h| h.title == cli_tip.title),
                        ))
                        .child(
                            Badge::new(format!("{param_count} synced params"))
                                .variant(BadgeVariant::Subtle)
                                .build(k),
                        ),
                )
                .child(
                    Button::new(if is_copied {
                        "Copied!"
                    } else {
                        "Copy Commands"
                    })
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
        .child(code_block(k, &examples));

    let skill_card = settings_card(k, "settings-skill-card")
        .child(settings_row(
            k,
            "Official Agent Skill Path",
            skill_tip,
            s.hovered_info,
            text("skills/rune-darkroom/SKILL.md")
                .font(ty::mono_compact(k))
                .color(k.c.text_accent),
        ))
        .child(row_hairline(k))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    text("Resolved `rd` Binary")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                )
                .child(
                    text(rd_path)
                        .font(ty::mono_compact(k))
                        .color(k.c.text_base)
                        .truncate(),
                ),
        );

    col()
        .w_full()
        .gap(space::CARD_GAP)
        .child(cli_card)
        .child(skill_card)
}

fn shortcuts_and_workflow_section(s: &AppState, k: &Kit) -> El {
    let settings_path_str = McpSettings::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/rune-darkroom/settings.json".to_owned());

    let watching = s.hot_folder_watcher.is_some();
    let watch_label = s
        .hot_folder_dir
        .as_ref()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "Idle — click Watch Folder… to auto-ingest arrivals".to_owned());

    let mut style_pills = row().w_full().gap(space::PILL_GAP);
    let none_active = s.hot_folder_style.is_none();
    style_pills = style_pills.child(
        row()
            .key("hot-style-none")
            .role("radio")
            .checked(none_active)
            .cursor(Cursor::Pointer)
            .h(size::PILL_COMPACT_H)
            .px(space::LG)
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
                .px(space::LG)
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

    let mut margin_pills = row().w_full().gap(space::PILL_GAP);
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

    let hot_tip = InfoTip {
        title: "Hot Folder Auto-Ingest",
        shortcut: None,
        description: "Watches a tethered capture folder for newly arriving RAW/JPEG frames, automatically ingesting them into the active session with an optional style.",
        cli_key: "settings.json → hot_folder_style  ·  File → Session → Watch Hot Folder",
        tier: "Settings · Studio",
    };

    let margin_tip = InfoTip {
        title: "Viewport Proof Margin",
        shortcut: Some("⌘⇧M"),
        description: "Surrounds the photograph with a color-neutral Studio (48 pt) or Exhibition (84 pt) gallery mat to evaluate framing and tonal balance objectively.",
        cli_key: "settings.json → proof_margin (compact | studio | exhibition)",
        tier: "Settings · Proofing",
    };

    let hot_folder_card = settings_card(k, "settings-hot-folder-card")
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(space::CARD_GAP)
                .child(
                    row()
                        .items_center()
                        .gap(space::ROW_GAP)
                        .min_w(0.0)
                        .shrink()
                        .child(status_dot(k, watching))
                        .child(
                            col()
                                .gap(space::XXS)
                                .min_w(0.0)
                                .child(
                                    row()
                                        .items_center()
                                        .gap(space::SM)
                                        .child(
                                            text("Hot Folder & Proof Margin")
                                                .font(ty::dialog_title(k))
                                                .color(k.c.text_base),
                                        )
                                        .child(info_badge(
                                            k,
                                            hot_tip,
                                            s.hovered_info
                                                .is_some_and(|h| h.title == hot_tip.title),
                                        )),
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
                        .gap(space::PILL_GAP)
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
        .child(row_hairline(k))
        .child(
            col()
                .w_full()
                .gap(space::ROW_GAP)
                .child(
                    text("Auto-Apply Style on Arrival")
                        .font(ty::control_label(k))
                        .color(k.c.text_muted),
                )
                .child(style_pills),
        )
        .child(row_hairline(k))
        .child(
            col()
                .w_full()
                .gap(space::ROW_GAP)
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
                                    text("Proof Margin")
                                        .font(ty::control_label(k))
                                        .color(k.c.text_muted),
                                )
                                .child(info_badge(
                                    k,
                                    margin_tip,
                                    s.hovered_info.is_some_and(|h| h.title == margin_tip.title),
                                )),
                        )
                        .child(
                            text(format!("{} · ⌘⇧M", s.proof_margin.label()))
                                .font(ty::mono_compact(k))
                                .color(k.c.text_base),
                        ),
                )
                .child(margin_pills),
        );

    // 2. Custom Keyboard Shortcuts Card (Studio Airy grouped rows)
    let mut shortcut_groups = col().w_full().gap(space::GROUP_GAP);
    for cat in ShortcutCategory::ALL {
        let mut group_col = col().w_full().gap(space::ROW_GAP).child(
            text(cat.label())
                .font(ty::section_header(k))
                .color(k.c.text_faint),
        );

        let mut rows_box = col()
            .w_full()
            .radius(k.radius.md)
            .fill(k.c.bg_base)
            .border(border::HAIRLINE, k.c.border_muted);

        if cat == ShortcutCategory::Workflow {
            let items = [
                ("Cycle Proof Margin", "⌘⇧M"),
                ("Move Photo + Sidecar to Selects", "⌘J"),
                ("Compare Variant Reference", "⌘⏎"),
                ("Multi-Recipe Export", "⌘⇧E"),
                ("Edit With External Editor (16-bit TIFF)", "⌘⇧W"),
            ];
            for (idx, (label, chord)) in items.into_iter().enumerate() {
                if idx > 0 {
                    rows_box = rows_box.child(row_hairline(k));
                }
                rows_box = rows_box.child(
                    row()
                        .w_full()
                        .px(space::XL)
                        .py(space::ROW_GAP)
                        .items_center()
                        .justify_between()
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
            let actions: Vec<ShortcutAction> = ShortcutAction::ALL
                .into_iter()
                .filter(|a| a.category() == cat)
                .collect();
            for (idx, action) in actions.into_iter().enumerate() {
                if idx > 0 {
                    rows_box = rows_box.child(row_hairline(k));
                }
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

                rows_box = rows_box.child(
                    row()
                        .key(format!("shortcut-row-{}", action.id()))
                        .w_full()
                        .px(space::XL)
                        .py(space::ROW_GAP)
                        .items_center()
                        .justify_between()
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
        group_col = group_col.child(rows_box);
        shortcut_groups = shortcut_groups.child(group_col);
    }

    let shortcuts_tip = InfoTip {
        title: "Custom Keyboard Shortcuts",
        shortcut: None,
        description: "Click any keycap to rebind single-key darkroom shortcuts. Digits 0–5 stay reserved for star ratings. Changes persist atomically to settings.json.",
        cli_key: "~/.config/rune-darkroom/settings.json → shortcuts",
        tier: "Settings · Shortcuts",
    };

    let shortcuts_card = settings_card(k, "settings-shortcuts-card")
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
                            text("Keyboard Shortcuts")
                                .font(ty::dialog_title(k))
                                .color(k.c.text_base),
                        )
                        .child(info_badge(
                            k,
                            shortcuts_tip,
                            s.hovered_info
                                .is_some_and(|h| h.title == shortcuts_tip.title),
                        )),
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
        .child(
            text(format!("Persisted to {settings_path_str}"))
                .font(ty::mono_micro(k))
                .color(k.c.text_faint),
        )
        .child(shortcut_groups);

    col()
        .w_full()
        .gap(space::CARD_GAP)
        .child(hot_folder_card)
        .child(shortcuts_card)
}

fn security_section(s: &AppState, k: &Kit) -> El {
    let config_path = McpSettings::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/rune-darkroom/settings.json".to_owned());

    let loopback_tip = InfoTip {
        title: "Loopback-Only Binding (127.0.0.1)",
        shortcut: None,
        description: "The embedded Streamable HTTP server binds exclusively to 127.0.0.1 and is never exposed to Wi-Fi, LAN, or external network interfaces.",
        cli_key: "crates/mcp/src/http.rs  ·  127.0.0.1 bind",
        tier: "Security · Network",
    };
    let origin_tip = InfoTip {
        title: "MCP 2025-11-25 Origin & Host Guard",
        shortcut: None,
        description: "Strict RFC 6454 Origin and Host header validation blocks DNS rebinding and cross-site requests immediately with 403 Forbidden.",
        cli_key: "MCP 2025-11-25  ·  /dev/urandom session tokens",
        tier: "Security · Auth",
    };
    let originals_tip = InfoTip {
        title: "Inviolable Originals & ⌘Z Undo",
        shortcut: Some("⌘Z"),
        description: "Source RAW/raster originals are never overwritten (`guard_not_source`, RAW extension blocklist, atomic temp-dotfile + sync_all + rename sidecars).",
        cli_key: "guard_not_source  ·  *.rune-edit.json atomic writes",
        tier: "Security · Storage",
    };

    let mut security_card = settings_card(k, "settings-security-card")
        .child(
            row()
                .items_center()
                .gap(space::ROW_GAP)
                .child(
                    icon(&icons::LOCK, size::ICON_LG)
                        .color(k.c.text_base)
                        .a11y_hidden(),
                )
                .child(
                    text("Local MCP Security & Guarantees")
                        .font(ty::dialog_title(k))
                        .color(k.c.text_base),
                ),
        )
        .child(row_hairline(k))
        .child(settings_row(
            k,
            "Loopback-Only Binding (127.0.0.1)",
            loopback_tip,
            s.hovered_info,
            Badge::new("Enforced")
                .variant(BadgeVariant::Subtle)
                .build(k),
        ))
        .child(row_hairline(k))
        .child(settings_row(
            k,
            "MCP 2025-11-25 Origin & Host Guard",
            origin_tip,
            s.hovered_info,
            Badge::new("403 Strict")
                .variant(BadgeVariant::Subtle)
                .build(k),
        ))
        .child(row_hairline(k))
        .child(settings_row(
            k,
            "Inviolable Originals & Atomic Sidecars",
            originals_tip,
            s.hovered_info,
            Badge::new("Atomic Write")
                .variant(BadgeVariant::Subtle)
                .build(k),
        ))
        .child(advanced_disclosure_row(
            k,
            "settings-disc-invariants",
            "Security & Sandbox Invariants",
            s.settings_security_invariants_open,
            0,
            4,
            |s| s.settings_security_invariants_open = !s.settings_security_invariants_open,
        ));

    if s.settings_security_invariants_open {
        let details = [
            "• 127.0.0.1-only TCP bind with bounded request reads & concurrency limits",
            "• Strict RFC 6454 Origin + u16 port Host validation (DNS rebinding guard)",
            "• Cryptographic /dev/urandom session IDs without empty-ID DELETE bypass",
            "• RAW extension blocklist, overwrite: false default, and atomic dotfile + sync_all + rename",
        ];
        let mut box_col = col()
            .w_full()
            .gap(space::XS)
            .padding(space::XL)
            .radius(k.radius.md)
            .fill(k.c.bg_deep)
            .border(border::HAIRLINE, k.c.border_muted);
        for line in details {
            box_col = box_col.child(text(line).font(ty::caption(k)).color(k.c.text_muted).wrap());
        }
        security_card = security_card.child(box_col);
    }

    let storage_card = settings_card(k, "settings-storage-card").child(
        row()
            .w_full()
            .items_center()
            .justify_between()
            .gap(space::ROW_GAP)
            .child(
                text("Settings File")
                    .font(ty::control_label_medium(k))
                    .color(k.c.text_base),
            )
            .child(
                text(config_path)
                    .font(ty::mono_compact(k))
                    .color(k.c.text_muted)
                    .truncate(),
            ),
    );

    col()
        .w_full()
        .gap(space::CARD_GAP)
        .child(security_card)
        .child(storage_card)
}

/// Unified `Studio Airy` macOS System Settings card (`layer_01`, `CARD_PAD = 14pt`, hairline border).
fn settings_card(k: &Kit, key: &'static str) -> El {
    col()
        .key(key)
        .w_full()
        .padding(space::CARD_PAD)
        .gap(space::CARD_HEADER_GAP)
        .radius(k.radius.lg)
        .continuous_corners()
        .fill(k.c.layer_01)
        .border(border::HAIRLINE, k.c.border_base)
}

/// Concise single-line Settings row with a trailing `ⓘ` hover info badge and right-aligned control.
fn settings_row(
    k: &Kit,
    label: &'static str,
    tip: InfoTip,
    hovered_info: Option<InfoTip>,
    trailing: El,
) -> El {
    row()
        .w_full()
        .items_center()
        .justify_between()
        .gap(space::CARD_GAP)
        .child(
            row()
                .items_center()
                .gap(space::SM)
                .child(
                    text(label)
                        .font(ty::control_label_medium(k))
                        .color(k.c.text_base),
                )
                .child(info_badge(
                    k,
                    tip,
                    hovered_info.is_some_and(|h| h.title == tip.title),
                )),
        )
        .child(trailing)
}

fn row_hairline(k: &Kit) -> El {
    let _ = Divider::horizontal();
    el().w_full()
        .h(border::HAIRLINE)
        .fill(k.c.border_muted)
        .a11y_hidden()
}

fn code_block(k: &Kit, code: &str) -> El {
    col()
        .w_full()
        .px(space::CARD_PAD)
        .py(space::XL)
        .radius(k.radius.md)
        .continuous_corners()
        .fill(color::code_surface(k))
        .border(border::HAIRLINE, k.c.border_base)
        .child(text(code).font(ty::mono_compact(k)).color(k.c.text_base))
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
            "Non-destructive · Camera profile, preset blend, patch, auto WB/tone, ⌘Z undo",
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
            "Non-destructive · Variants, user/stacked styles, profile_list, clipboard",
        ),
        (
            "rune_edit_layers",
            "Non-destructive · Adjustment layers, composable masks, AI subject/bg, heal/clone",
        ),
    ];
    let mut list = col()
        .w_full()
        .radius(k.radius.md)
        .fill(k.c.bg_base)
        .border(border::HAIRLINE, k.c.border_muted);
    for (idx, (name, desc)) in items.into_iter().enumerate() {
        if idx > 0 {
            list = list.child(row_hairline(k));
        }
        list = list.child(
            row()
                .w_full()
                .px(space::XL)
                .py(space::ROW_GAP)
                .items_center()
                .justify_between()
                .gap(space::CARD_GAP)
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
