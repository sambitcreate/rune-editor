//! Local Model Context Protocol (`2025-11-25`) server for Rune Darkroom.
//!
//! Provides BOTH official local transports over a single shared 7-tool engine:
//! - **Streamable HTTP (`http://127.0.0.1:<port>/mcp`)** embedded inside the Rune Darkroom
//!   desktop GUI (toggleable ON/OFF in Settings) so agents can edit the live canvas on the
//!   user's behalf with `⌘Z` undo support.
//! - **Stdio (`rd mcp`)** for CLI/desktop agents that spawn a local command directly.

pub mod http;
pub mod protocol;
pub mod stdio;
pub mod tools;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use http::{LocalMcpServer, is_allowed_local_host, is_allowed_local_origin};
pub use protocol::{
    MCP_PROTOCOL_LATEST, SUPPORTED_PROTOCOL_VERSIONS, is_supported_protocol_version,
};
pub use stdio::{MAX_STDIO_LINE_BYTES, serve_stdio};
pub use tools::{
    ActiveCanvasSnapshot, HeadlessHostBridge, McpHostBridge, McpService, tool_definitions,
};

/// Default localhost port for Rune Darkroom's embedded Streamable HTTP MCP server.
pub const DEFAULT_MCP_PORT: u16 = 37420;

/// Persistent user settings for Rune Darkroom's local MCP server (`~/.config/rune-darkroom/settings.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    /// Whether the local Streamable HTTP MCP server is enabled in the desktop app.
    pub enabled: bool,
    /// Localhost TCP port (`127.0.0.1:<port>`, default `37420`).
    pub port: u16,
    /// When `true`, MCP agents may apply live non-destructive edits directly to the open canvas.
    pub allow_live_canvas_edits: bool,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_MCP_PORT,
            allow_live_canvas_edits: true,
        }
    }
}

impl McpSettings {
    pub fn config_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".config")
                .join("rune-darkroom")
                .join("settings.json"),
        )
    }

    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<PathBuf, String> {
        let path = Self::config_path()
            .ok_or_else(|| "Could not determine HOME directory for settings".to_owned())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        rune_editor_core::session::atomic_write_bytes(&path, json.as_bytes())
            .map_err(|e| e.to_string())?;
        Ok(path)
    }

    pub fn endpoint_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    /// Copy-ready Streamable HTTP MCP client JSON snippet (Antigravity, Claude Code, Cursor, Windsurf).
    pub fn http_client_snippet(&self) -> String {
        format!(
            "{{\n  \"mcpServers\": {{\n    \"rune-darkroom\": {{\n      \"url\": \"http://127.0.0.1:{}/mcp\"\n    }}\n  }}\n}}",
            self.port
        )
    }

    /// Copy-ready Stdio MCP client JSON snippet (Claude Desktop, Cursor, OpenCode, Codex).
    pub fn stdio_client_snippet(rd_binary_path: &str) -> String {
        format!(
            "{{\n  \"mcpServers\": {{\n    \"rune-darkroom\": {{\n      \"command\": \"{rd_binary_path}\",\n      \"args\": [\"mcp\"]\n    }}\n  }}\n}}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Cursor, Read, Write};
    use std::net::TcpStream;
    use std::sync::Arc;

    use serde_json::{Value, json};

    fn http_raw(port: u16, raw_req: &str) -> (String, Vec<(String, String)>, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect local mcp");
        s.write_all(raw_req.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut reader = BufReader::new(s);
        let mut status = String::new();
        reader.read_line(&mut status).unwrap();
        let mut headers = Vec::new();
        let mut content_len = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let t = line.trim_end_matches(['\r', '\n']);
            if t.is_empty() {
                break;
            }
            if let Some((k, v)) = t.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
                headers.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
        let mut body = vec![0u8; content_len];
        reader.read_exact(&mut body).unwrap();
        (
            status.trim().to_owned(),
            headers,
            String::from_utf8_lossy(&body).into_owned(),
        )
    }

    #[test]
    fn origin_validation_blocks_remote_and_allows_localhost() {
        assert!(is_allowed_local_origin("http://localhost"));
        assert!(is_allowed_local_origin("http://localhost:6274"));
        assert!(is_allowed_local_origin("http://127.0.0.1:37420"));
        assert!(is_allowed_local_origin("http://[::1]:5173"));
        assert!(is_allowed_local_origin("https://localhost:8443"));
        assert!(!is_allowed_local_origin("https://evil.example.com"));
        assert!(!is_allowed_local_origin("http://localhost.evil.com"));
        assert!(!is_allowed_local_origin("http://localhost:80@evil.example"));
        assert!(!is_allowed_local_origin("http://127.0.0.1:anything"));
        assert!(!is_allowed_local_origin("http://127.0.0.1:+80"));
        assert!(!is_allowed_local_origin("http://127.0.0.1:99999"));
        assert!(!is_allowed_local_origin("http://localhost/path"));
        assert!(!is_allowed_local_origin("http://localhost:80?q=1"));
        assert!(!is_allowed_local_origin("http://localhost:80#frag"));
        assert!(!is_allowed_local_origin("null"));

        assert!(is_allowed_local_host("localhost"));
        assert!(is_allowed_local_host("localhost:37420"));
        assert!(is_allowed_local_host("127.0.0.1:37420"));
        assert!(is_allowed_local_host("[::1]:37420"));
        assert!(!is_allowed_local_host("localhost:80@evil.example"));
        assert!(!is_allowed_local_host("127.0.0.1:anything"));
        assert!(!is_allowed_local_host("127.0.0.1:+80"));
        assert!(!is_allowed_local_host("localhost:"));
        assert!(!is_allowed_local_host("[::1]:notaport"));
    }

    #[test]
    fn streamable_http_end_to_end_spec_compliance() {
        let svc = Arc::new(McpService::new(Arc::new(HeadlessHostBridge)));
        let mut server = LocalMcpServer::start(0, svc).expect("bind ephemeral port");
        let port = server.port();

        // 1. Untrusted Origin MUST be rejected with 403 Forbidden (DNS-rebinding guard)
        let init_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-11-25" }
        })
        .to_string();
        let bad_req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://attacker.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{init_body}",
            init_body.len()
        );
        let (status, _, _) = http_raw(port, &bad_req);
        assert!(
            status.contains("403"),
            "expected 403 for untrusted origin, got {status}"
        );

        // 2. GET /mcp MUST return 405 Method Not Allowed per 2025-11-25 spec
        let get_req = format!("GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        let (status, _, _) = http_raw(port, &get_req);
        assert!(
            status.contains("405"),
            "expected 405 for GET /mcp, got {status}"
        );

        // 3. Valid POST /mcp initialize returns 200, negotiated version, and Mcp-Session-Id
        let ok_init = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nMCP-Protocol-Version: 2025-11-25\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{init_body}",
            init_body.len()
        );
        let (status, headers, body) = http_raw(port, &ok_init);
        assert!(
            status.contains("200"),
            "expected 200 on initialize, got {status}"
        );
        let sid = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("mcp-session-id"))
            .map(|(_, v)| v.clone())
            .expect("Mcp-Session-Id header must be present");
        assert!(sid.bytes().all(|b| (0x21..=0x7E).contains(&b)));
        let init_json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(init_json["result"]["protocolVersion"], "2025-11-25");

        // 4. Notification (`notifications/initialized`) returns 202 Accepted
        let notif = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string();
        let notif_req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nMcp-Session-Id: {sid}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{notif}",
            notif.len()
        );
        let (status, _, _) = http_raw(port, &notif_req);
        assert!(
            status.contains("202"),
            "expected 202 on notification, got {status}"
        );

        // 5. tools/list returns all 9 annotated tools
        let list_body = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string();
        let list_req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nMcp-Session-Id: {sid}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{list_body}",
            list_body.len()
        );
        let (status, _, body) = http_raw(port, &list_req);
        assert!(status.contains("200"));
        let list_json: Value = serde_json::from_str(&body).unwrap();
        let tools = list_json["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 9);
        assert!(
            tools
                .iter()
                .all(|t| t.get("annotations").is_some() && t.get("outputSchema").is_some())
        );

        // 6. Spoofed Host header MUST be rejected with 403 Forbidden (DNS-rebinding defense)
        let bad_host = format!(
            "POST /mcp HTTP/1.1\r\nHost: evil.attacker.test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{list_body}",
            list_body.len()
        );
        let (status, _, _) = http_raw(port, &bad_host);
        assert!(
            status.contains("403"),
            "expected 403 for spoofed Host, got {status}"
        );

        // 7. Chunked Transfer-Encoding POST works cleanly
        let chunk_req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nMcp-Session-Id: {sid}\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n{:x}\r\n{list_body}\r\n0\r\n\r\n",
            list_body.len()
        );
        let (status, _, body) = http_raw(port, &chunk_req);
        assert!(
            status.contains("200"),
            "expected 200 on chunked POST, got {status}"
        );
        let chunk_json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(chunk_json["result"]["tools"].as_array().unwrap().len(), 9);

        // 8. DELETE /mcp terminates the session cleanly
        let del_req = format!(
            "DELETE /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nMcp-Session-Id: {sid}\r\n\r\n"
        );
        let (status, _, _) = http_raw(port, &del_req);
        assert!(
            status.contains("204"),
            "expected 204 on DELETE, got {status}"
        );

        server.stop();
    }

    #[test]
    fn agent_field_feedback_regressions_curves_wheels_prefixes_patch_and_keepalive() {
        use rune_editor_core::{CullFlag, DevelopRecipe};
        use rune_editor_decode::generate_demo_hdr_scene;
        use std::sync::Mutex;

        struct MockGuiHost {
            snap: Mutex<ActiveCanvasSnapshot>,
        }
        impl McpHostBridge for MockGuiHost {
            fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
                Some(self.snap.lock().unwrap().clone())
            }
            fn apply_live_recipe(
                &self,
                _target: Option<&std::path::Path>,
                new_recipe: DevelopRecipe,
                _summary: String,
            ) -> Result<bool, String> {
                self.snap.lock().unwrap().recipe = new_recipe;
                Ok(true)
            }
        }

        let img = Arc::new(generate_demo_hdr_scene(160, 120));
        let host = Arc::new(MockGuiHost {
            snap: Mutex::new(ActiveCanvasSnapshot {
                display_name: "demo.jpg".to_owned(),
                source_path: None,
                image: img,
                recipe: DevelopRecipe::default(),
                rating: 0,
                flag: CullFlag::None,
                color_tag: rune_editor_core::ColorTag::None,
                demo_mode: true,
                folder: None,
                catalog_len: 1,
            }),
        });
        let svc = Arc::new(McpService::new(host));
        let mut server = LocalMcpServer::start(0, svc.clone()).expect("bind");
        let port = server.port();

        // 1. Unknown top-level argument must fail loudly (never silent ok:true no-op!)
        let bad_top = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(1)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "rune_edit_photo",
                    "arguments": { "wheels": { "shadows": { "hue_x": -0.2 } } }
                })),
            })
            .unwrap();
        assert_eq!(bad_top["result"]["isError"], true);

        // 2. Redundant prefixes (levels.levels.in_black, bw.bw.enabled, wheels.shadows_wheel.hue_x)
        //    AND tone curves in all 4 formats (pair array, flat array, keyed map, CSV string) work via `set`!
        let before = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(2)),
                method: "tools/call".to_owned(),
                params: Some(
                    json!({ "name": "rune_analyze_photo", "arguments": { "fast": true } }),
                ),
            })
            .unwrap();
        let before_sh = before["result"]["structuredContent"]["analysis"]["color"]["cast_by_tone_ab"]["shadows"].clone();

        let edit_set = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(3)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "rune_edit_photo",
                    "arguments": {
                        "set": {
                            "levels.levels.in_black": 0.03,
                            "curves.curves.luma": [[0.0, 0.0], [0.25, 0.20], [0.75, 0.82], [1.0, 1.0]],
                            "curves.red": [0.0, 0.0, 0.5, 0.54, 1.0, 1.0],
                            "curves.green": { "0": 0.0, "0.5": 0.5, "1": 1.0 },
                            "curves.blue": "0,0;0.5,0.48;1,1",
                            "wheels.shadows_wheel.hue_x": -0.25,
                            "wheels.shadows_wheel.hue_y": -0.25
                        }
                    }
                })),
            })
            .unwrap();
        assert_eq!(
            edit_set["result"]["isError"], false,
            "set failed: {edit_set}"
        );

        let after = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(4)),
                method: "tools/call".to_owned(),
                params: Some(
                    json!({ "name": "rune_analyze_photo", "arguments": { "fast": true } }),
                ),
            })
            .unwrap();
        let after_sh = after["result"]["structuredContent"]["analysis"]["color"]["cast_by_tone_ab"]
            ["shadows"]
            .clone();
        assert_ne!(
            before_sh, after_sh,
            "shadows_wheel MUST shift analyzed shadow cast"
        );

        // 3. `patch` accepts grouped `wheels` AND bare curve arrays!
        let edit_patch = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(5)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "rune_edit_photo",
                    "arguments": {
                        "patch": {
                            "wheels": { "shadows": { "hue_x": 0.2, "hue_y": 0.1 } },
                            "curves": { "luma": [[0.0, 0.02], [0.5, 0.52], [1.0, 0.98]] }
                        }
                    }
                })),
            })
            .unwrap();
        assert_eq!(
            edit_patch["result"]["isError"], false,
            "patch failed: {edit_patch}"
        );

        // 4. Persistent HTTP/1.1 Keep-Alive: 3 sequential requests over ONE TcpStream (zero ECONNRESET!)
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let mut reader = BufReader::new(s.try_clone().unwrap());
        for id in 1..=3 {
            let body = json!({ "jsonrpc": "2.0", "id": id, "method": "ping" }).to_string();
            let req = format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: keep-alive\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            s.write_all(req.as_bytes()).unwrap();
            s.flush().unwrap();
            let mut status = String::new();
            reader.read_line(&mut status).unwrap();
            assert!(
                status.contains("200"),
                "keep-alive req #{id} failed: {status}"
            );
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                let t = h.trim_end_matches(['\r', '\n']);
                if t.is_empty() {
                    break;
                }
                if let Some((k, v)) = t.split_once(':')
                    && k.eq_ignore_ascii_case("content-length")
                {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf).unwrap();
            let resp: Value = serde_json::from_slice(&buf).unwrap();
            assert_eq!(resp["id"], id);
        }

        server.stop();
    }

    #[test]
    fn stdio_transport_handles_initialize_and_schema_tool() {
        let svc = McpService::new(Arc::new(HeadlessHostBridge));
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"rune_get_schema\",\"arguments\":{}}}\n"
        );
        let mut out = Vec::new();
        serve_stdio(&svc, Cursor::new(input.as_bytes()), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "notification produces no output line");
        let r2: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r2["result"]["isError"], false);
        assert!(r2["result"]["structuredContent"]["schema"]["parameters"].is_array());
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rune_mcp_test_{label}_{}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    #[test]
    fn http_hardening_rejects_host_origin_framing_session_and_protocol_attacks() {
        let svc = Arc::new(McpService::new(Arc::new(HeadlessHostBridge)));
        let mut server = LocalMcpServer::start(0, svc).expect("bind ephemeral port");
        let port = server.port();

        let ping_body = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string();

        // 1. Userinfo/port bypass in Origin & Host -> 403 Forbidden
        let evil_origin = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://localhost:80@evil.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{ping_body}",
            ping_body.len()
        );
        let (status, _, _) = http_raw(port, &evil_origin);
        assert!(
            status.contains("403"),
            "expected 403 for localhost:80@evil.example Origin, got {status}"
        );

        let evil_host = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:anything\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{ping_body}",
            ping_body.len()
        );
        let (status, _, _) = http_raw(port, &evil_host);
        assert!(
            status.contains("403"),
            "expected 403 for 127.0.0.1:anything Host, got {status}"
        );

        // 2. Missing Host on HTTP/1.1 -> 400 Bad Request
        let missing_host = format!(
            "POST /mcp HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{ping_body}",
            ping_body.len()
        );
        let (status, _, _) = http_raw(port, &missing_host);
        assert!(
            status.contains("400"),
            "expected 400 for missing Host on HTTP/1.1, got {status}"
        );

        // 3. Dual Transfer-Encoding + Content-Length -> 400 Bad Request
        let dual_te_cl = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nTransfer-Encoding: chunked\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{:x}\r\n{ping_body}\r\n0\r\n\r\n",
            ping_body.len(),
            ping_body.len()
        );
        let (status, _, _) = http_raw(port, &dual_te_cl);
        assert!(
            status.contains("400"),
            "expected 400 for dual Transfer-Encoding + Content-Length, got {status}"
        );

        // 3b. Duplicate Content-Length -> 400 Bad Request
        let dup_cl = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{ping_body}",
            ping_body.len(),
            ping_body.len()
        );
        let (status, _, _) = http_raw(port, &dup_cl);
        assert!(
            status.contains("400"),
            "expected 400 for duplicate Content-Length, got {status}"
        );

        // 4. Bad chunk trailing CRLF (`XX` instead of `\r\n`) -> 400 Bad Request
        let bad_chunk_crlf = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n{:x}\r\n{ping_body}XX0\r\n\r\n",
            ping_body.len()
        );
        let (status, _, _) = http_raw(port, &bad_chunk_crlf);
        assert!(
            status.contains("400"),
            "expected 400 for bad chunk CRLF, got {status}"
        );

        // 5. Unsupported MCP-Protocol-Version on POST -> 400 Bad Request
        let bad_proto = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nMCP-Protocol-Version: 1999-01-01\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{ping_body}",
            ping_body.len()
        );
        let (status, _, _) = http_raw(port, &bad_proto);
        assert!(
            status.contains("400"),
            "expected 400 for unsupported MCP-Protocol-Version, got {status}"
        );

        // 6. Invalid jsonrpc version -> 400 + JSON-RPC -32600
        let bad_rpc = json!({ "jsonrpc": "1.0", "id": 7, "method": "ping" }).to_string();
        let bad_rpc_req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{bad_rpc}",
            bad_rpc.len()
        );
        let (status, _, body) = http_raw(port, &bad_rpc_req);
        assert!(
            status.contains("400"),
            "expected 400 for invalid jsonrpc version, got {status}"
        );
        let err_json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            err_json["error"]["code"],
            protocol::rpc_err::INVALID_REQUEST
        );

        // 7. Terminated last session MUST return 404 Not Found (no is_empty() bypass!)
        let init_body = json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "initialize",
            "params": { "protocolVersion": "2025-11-25" }
        })
        .to_string();
        let init_req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{init_body}",
            init_body.len()
        );
        let (status, headers, _) = http_raw(port, &init_req);
        assert!(status.contains("200"), "initialize failed: {status}");
        let sid = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("mcp-session-id"))
            .map(|(_, v)| v.clone())
            .expect("Mcp-Session-Id header missing");
        assert!(sid.starts_with("rd-") && sid.len() >= 35);

        let del_req = format!(
            "DELETE /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nMcp-Session-Id: {sid}\r\n\r\n"
        );
        let (status, _, _) = http_raw(port, &del_req);
        assert!(status.contains("204"), "DELETE failed: {status}");

        let reuse_req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nMcp-Session-Id: {sid}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{ping_body}",
            ping_body.len()
        );
        let (status, _, _) = http_raw(port, &reuse_req);
        assert!(
            status.contains("404"),
            "reusing deleted last session MUST return 404 Not Found, got {status}"
        );

        server.stop();
    }

    #[test]
    fn mcp_out_clobber_guard_blocks_non_images_raw_source_and_overwrite_without_flag() {
        use rune_editor_core::{CullFlag, DevelopRecipe};
        use rune_editor_decode::generate_demo_hdr_scene;

        let dir = unique_test_dir("out_guard");
        let src_jpg = dir.join("source.jpg");
        let demo = generate_demo_hdr_scene(64, 48);
        rune_editor_decode::export_image(
            &demo,
            &DevelopRecipe::default(),
            &src_jpg,
            rune_editor_decode::ExportFormat::Jpeg { quality: 85 },
        )
        .expect("write source jpg");

        struct SinglePhotoHost {
            snap: ActiveCanvasSnapshot,
        }
        impl McpHostBridge for SinglePhotoHost {
            fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
                Some(self.snap.clone())
            }
        }

        let svc = McpService::new(Arc::new(SinglePhotoHost {
            snap: ActiveCanvasSnapshot {
                display_name: "source.jpg".to_owned(),
                source_path: Some(src_jpg.clone()),
                image: Arc::new(demo),
                recipe: DevelopRecipe::default(),
                rating: 0,
                flag: CullFlag::None,
                color_tag: rune_editor_core::ColorTag::None,
                demo_mode: false,
                folder: Some(dir.clone()),
                catalog_len: 1,
            },
        }));

        let call = |name: &str, arguments: Value| {
            svc.handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(1)),
                method: "tools/call".to_owned(),
                params: Some(json!({ "name": name, "arguments": arguments })),
            })
            .unwrap()
        };

        // 1. Reject `.zshrc` & dotfile `.zshrc`
        for bad_out in [dir.join("evil.zshrc"), dir.join(".zshrc")] {
            let r_prev = call("rune_preview_photo", json!({ "out": bad_out }));
            assert_eq!(
                r_prev["result"]["isError"], true,
                "preview must reject {bad_out:?}"
            );
            assert!(!bad_out.exists());

            let r_exp = call(
                "rune_cull_or_export",
                json!({ "action": "export", "out": bad_out }),
            );
            assert_eq!(
                r_exp["result"]["isError"], true,
                "export must reject {bad_out:?}"
            );
            assert!(!bad_out.exists());
        }

        // 2. Reject RAW `.dng` extension
        let dng_out = dir.join("clobber.dng");
        let r_prev_dng = call("rune_preview_photo", json!({ "out": dng_out }));
        assert_eq!(r_prev_dng["result"]["isError"], true);
        let r_exp_dng = call(
            "rune_cull_or_export",
            json!({ "action": "export", "out": dng_out }),
        );
        assert_eq!(r_exp_dng["result"]["isError"], true);

        // 3. Reject overwriting source photo (even with overwrite: true, via lexical `..` through
        //    non-existent subdirectories, or via symlinked parent dir!)
        let r_src = call(
            "rune_preview_photo",
            json!({ "out": src_jpg, "overwrite": true }),
        );
        assert_eq!(r_src["result"]["isError"], true);

        let dotdot_src = dir.join("nonexistent_subdir").join("..").join("source.jpg");
        let r_dotdot_prev = call(
            "rune_preview_photo",
            json!({ "out": dotdot_src, "overwrite": true }),
        );
        assert_eq!(
            r_dotdot_prev["result"]["isError"], true,
            "lexical `..` through non-existent subdir targeting source must be blocked on preview"
        );
        let r_dotdot_exp = call(
            "rune_cull_or_export",
            json!({ "action": "export", "out": dotdot_src, "overwrite": true }),
        );
        assert_eq!(
            r_dotdot_exp["result"]["isError"], true,
            "lexical `..` through non-existent subdir targeting source must be blocked on export"
        );

        #[cfg(unix)]
        {
            let link_dir = dir.join("symlink_parent");
            std::os::unix::fs::symlink(&dir, &link_dir).expect("create symlink parent");
            let via_link = link_dir.join("source.jpg");
            let r_link_prev = call(
                "rune_preview_photo",
                json!({ "out": via_link, "overwrite": true }),
            );
            assert_eq!(
                r_link_prev["result"]["isError"], true,
                "symlinked source overwrite must be blocked on preview"
            );
            let r_link_exp = call(
                "rune_cull_or_export",
                json!({ "action": "export", "out": via_link, "overwrite": true }),
            );
            assert_eq!(
                r_link_exp["result"]["isError"], true,
                "symlinked source overwrite must be blocked on export"
            );
        }

        // 4. Valid new `.jpg` succeeds, second call without `overwrite: true` fails, with `overwrite: true` succeeds!
        let valid_out = dir.join("preview_ok.jpg");
        let r_ok = call("rune_preview_photo", json!({ "out": valid_out }));
        assert_eq!(
            r_ok["result"]["isError"], false,
            "valid preview failed: {r_ok}"
        );
        assert!(valid_out.exists());

        let r_dup = call("rune_preview_photo", json!({ "out": valid_out }));
        assert_eq!(r_dup["result"]["isError"], true);
        assert_eq!(r_dup["result"]["structuredContent"]["code"], "FILE_EXISTS");

        let r_overwrite = call(
            "rune_preview_photo",
            json!({ "out": valid_out, "overwrite": true }),
        );
        assert_eq!(
            r_overwrite["result"]["isError"], false,
            "overwrite:true should succeed: {r_overwrite}"
        );

        let exp_out = dir.join("export_ok.jpg");
        std::fs::write(&exp_out, b"existing").unwrap();
        let r_exp_dup = call(
            "rune_cull_or_export",
            json!({ "action": "export", "out": exp_out }),
        );
        assert_eq!(r_exp_dup["result"]["isError"], true);
        assert_eq!(
            r_exp_dup["result"]["structuredContent"]["code"],
            "FILE_EXISTS"
        );

        let r_exp_ow = call(
            "rune_cull_or_export",
            json!({ "action": "export", "out": exp_out, "overwrite": true }),
        );
        assert_eq!(
            r_exp_ow["result"]["isError"], false,
            "export overwrite:true should succeed: {r_exp_ow}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_mcp_edit_preserves_existing_sidecar_history_and_revision() {
        use rune_editor_core::{CullFlag, DevelopRecipe, EditActor, PhotoMeta};
        use rune_editor_decode::generate_demo_hdr_scene;
        use std::sync::Mutex;

        let dir = unique_test_dir("live_provenance");
        let photo_path = dir.join("portrait.jpg");
        let demo = generate_demo_hdr_scene(64, 48);
        rune_editor_decode::export_image(
            &demo,
            &DevelopRecipe::default(),
            &photo_path,
            rune_editor_decode::ExportFormat::Jpeg { quality: 85 },
        )
        .expect("write photo");

        // Pre-seed sidecar with revision=5 and 2 prior history entries.
        let mut seed = PhotoMeta {
            revision: 4,
            rating: 4,
            flag: CullFlag::Pick,
            recipe: DevelopRecipe {
                exposure: 0.25,
                ..DevelopRecipe::default()
            },
            ..PhotoMeta::default()
        };
        seed.push_history(
            EditActor::Human,
            "Initial base grade",
            vec![],
            Some(DevelopRecipe::default()),
        );
        seed.push_history(
            EditActor::Human,
            "Lift exposure +0.25",
            vec![],
            Some(DevelopRecipe::default()),
        );
        seed.save_sidecar(&photo_path).expect("save seeded sidecar");
        let before_disk = PhotoMeta::load_sidecar(&photo_path)
            .expect("load seeded")
            .expect("sidecar exists");
        assert_eq!(before_disk.revision, 5);
        assert_eq!(before_disk.history.len(), 2);

        struct LiveHost {
            snap: Mutex<ActiveCanvasSnapshot>,
        }
        impl McpHostBridge for LiveHost {
            fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
                Some(self.snap.lock().unwrap().clone())
            }
            fn apply_live_recipe(
                &self,
                _target: Option<&std::path::Path>,
                new_recipe: DevelopRecipe,
                _summary: String,
            ) -> Result<bool, String> {
                self.snap.lock().unwrap().recipe = new_recipe;
                Ok(true)
            }
        }

        let svc = McpService::new(Arc::new(LiveHost {
            snap: Mutex::new(ActiveCanvasSnapshot {
                display_name: "portrait.jpg".to_owned(),
                source_path: Some(photo_path.clone()),
                image: Arc::new(demo),
                recipe: before_disk.recipe.clone(),
                rating: 4,
                flag: CullFlag::Pick,
                color_tag: rune_editor_core::ColorTag::None,
                demo_mode: false,
                folder: Some(dir.clone()),
                catalog_len: 1,
            }),
        }));

        // Edit live canvas with `path` omitted!
        let resp = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(1)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "rune_edit_photo",
                    "arguments": {
                        "patch": { "contrast": 0.2 }
                    }
                })),
            })
            .unwrap();
        assert_eq!(resp["result"]["isError"], false, "live edit failed: {resp}");

        let after_disk = PhotoMeta::load_sidecar(&photo_path)
            .expect("reload sidecar")
            .expect("sidecar present");
        assert_eq!(
            after_disk.revision, 6,
            "live MCP edit must increment existing revision 5 -> 6, not reset to 1"
        );
        assert_eq!(
            after_disk.history.len(),
            3,
            "live MCP edit must preserve the 2 existing history entries and append #3"
        );
        assert_eq!(after_disk.history[0].summary, "Initial base grade");
        assert_eq!(after_disk.history[1].summary, "Lift exposure +0.25");
        assert!((after_disk.recipe.contrast - 0.2).abs() < 1e-4);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_wb_pick_and_diff_assertions_work_end_to_end() {
        use rune_editor_core::{CullFlag, DevelopRecipe};
        use rune_editor_decode::generate_demo_hdr_scene;
        use std::sync::Mutex;

        struct LiveHost {
            snap: Mutex<ActiveCanvasSnapshot>,
        }
        impl McpHostBridge for LiveHost {
            fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
                Some(self.snap.lock().unwrap().clone())
            }
            fn apply_live_recipe(
                &self,
                _target: Option<&std::path::Path>,
                new_recipe: DevelopRecipe,
                _summary: String,
            ) -> Result<bool, String> {
                self.snap.lock().unwrap().recipe = new_recipe;
                Ok(true)
            }
        }

        let demo = Arc::new(generate_demo_hdr_scene(120, 80));
        let host = Arc::new(LiveHost {
            snap: Mutex::new(ActiveCanvasSnapshot {
                display_name: "demo.dng".to_owned(),
                source_path: None,
                image: demo,
                recipe: DevelopRecipe::default(),
                rating: 0,
                flag: CullFlag::None,
                color_tag: rune_editor_core::ColorTag::None,
                demo_mode: true,
                folder: None,
                catalog_len: 1,
            }),
        });
        let svc = McpService::new(host.clone());

        // 1. Out-of-range wb_pick fails validation
        let bad_wb = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(1)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "rune_edit_photo",
                    "arguments": { "wb_pick": [1.5, 0.5] }
                })),
            })
            .unwrap();
        assert_eq!(bad_wb["result"]["isError"], true);

        // 2. Valid wb_pick samples 3x3 patch and updates temperature/tint
        let ok_wb = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(2)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "rune_edit_photo",
                    "arguments": { "wb_pick": [0.68, 0.34] }
                })),
            })
            .unwrap();
        assert_eq!(ok_wb["result"]["isError"], false, "wb_pick failed: {ok_wb}");
        let rec = host.snap.lock().unwrap().recipe.clone();
        assert!(
            rec.temperature.abs() > 1e-3 || rec.tint.abs() > 1e-3,
            "wb_pick on warm sun spot should shift WB (temp={}, tint={})",
            rec.temperature,
            rec.tint
        );

        // 3. Push exposure hard so diff delta_e2000.mean is large, then test passing & failing assertions
        let _ = svc.handle_request(&protocol::JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(3)),
            method: "tools/call".to_owned(),
            params: Some(json!({
                "name": "rune_edit_photo",
                "arguments": { "patch": { "exposure": 2.5 } }
            })),
        });

        let diff_pass = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(4)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "rune_diff_photo",
                    "arguments": {
                        "assertions": {
                            "max_highlight_clip_pct": 100.0,
                            "max_shadow_clip_pct": 100.0,
                            "max_skin_de2000": 100.0,
                            "max_mean_de2000": 100.0
                        }
                    }
                })),
            })
            .unwrap();
        assert_eq!(diff_pass["result"]["isError"], false);
        assert_eq!(
            diff_pass["result"]["structuredContent"]["assertions_passed"],
            true
        );
        assert!(
            diff_pass["result"]["structuredContent"]["violations"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let diff_fail = svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(5)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "rune_diff_photo",
                    "arguments": {
                        "assertions": {
                            "max_mean_de2000": 0.01
                        }
                    }
                })),
            })
            .unwrap();
        assert_eq!(diff_fail["result"]["isError"], true);
        assert_eq!(
            diff_fail["result"]["structuredContent"]["assertions_passed"],
            false
        );
        assert!(
            !diff_fail["result"]["structuredContent"]["violations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn stdio_rejects_invalid_jsonrpc_version() {
        let svc = McpService::new(Arc::new(HeadlessHostBridge));
        let input = "{\"jsonrpc\":\"1.0\",\"id\":1,\"method\":\"ping\"}\n";
        let mut out = Vec::new();
        serve_stdio(&svc, Cursor::new(input.as_bytes()), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let resp: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(resp["error"]["code"], protocol::rpc_err::INVALID_REQUEST);
    }

    #[test]
    fn mcp_phase1_color_tone_detail_parity_end_to_end() {
        use rune_editor_core::{
            ClarityMethod, CullFlag, DevelopRecipe, FilmCurve, GrainType, WbPreset,
        };
        use rune_editor_decode::generate_demo_hdr_scene;
        use std::sync::Mutex;

        struct LiveHost {
            snap: Mutex<ActiveCanvasSnapshot>,
        }
        impl McpHostBridge for LiveHost {
            fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
                Some(self.snap.lock().unwrap().clone())
            }
            fn apply_live_recipe(
                &self,
                _target: Option<&std::path::Path>,
                new_recipe: DevelopRecipe,
                _summary: String,
            ) -> Result<bool, String> {
                self.snap.lock().unwrap().recipe = new_recipe;
                Ok(true)
            }
        }

        let demo = Arc::new(generate_demo_hdr_scene(120, 80));
        let host = Arc::new(LiveHost {
            snap: Mutex::new(ActiveCanvasSnapshot {
                display_name: "phase1.dng".to_owned(),
                source_path: None,
                image: demo,
                recipe: DevelopRecipe::default(),
                rating: 0,
                flag: CullFlag::None,
                color_tag: rune_editor_core::ColorTag::None,
                demo_mode: true,
                folder: None,
                catalog_len: 1,
            }),
        });
        let svc = McpService::new(host.clone());

        let call = |id: u64, name: &str, arguments: Value| {
            svc.handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(id)),
                method: "tools/call".to_owned(),
                params: Some(json!({ "name": name, "arguments": arguments })),
            })
            .unwrap()
        };

        // 1. `rune_get_schema` exposes every Phase 1 parameter ID.
        let schema_resp = call(1, "rune_get_schema", json!({}));
        assert_eq!(schema_resp["result"]["isError"], false);
        let params = schema_resp["result"]["structuredContent"]["schema"]["parameters"]
            .as_array()
            .expect("schema parameters array");
        for expected in [
            "film_curve",
            "wb_preset",
            "curves.rgb",
            "levels.red.in_black",
            "levels.green.in_mid",
            "levels.blue.out_white",
            "master_wheel.hue_x",
            "bw.split_shadow_hue",
            "bw.split_shadow_sat",
            "bw.split_highlight_hue",
            "bw.split_highlight_sat",
            "clarity_method",
            "sharpen_radius",
            "sharpen_threshold",
            "sharpen_halo",
            "nr_color",
            "grain_type",
            "grain_size",
        ] {
            assert!(
                params.iter().any(|p| p["id"] == expected),
                "rune_get_schema missing Phase 1 parameter '{expected}'"
            );
        }

        // 2. `rune_inspect_photo` includes Kelvin WB readout (`5500 K` at neutral).
        let insp0 = call(2, "rune_inspect_photo", json!({}));
        assert_eq!(insp0["result"]["isError"], false);
        assert_eq!(insp0["result"]["structuredContent"]["wb_kelvin"], 5500);
        assert_eq!(insp0["result"]["structuredContent"]["wb_preset"], "as_shot");
        assert_eq!(
            insp0["result"]["structuredContent"]["film_curve"],
            "film_standard"
        );

        // 3. Invalid top-level `wb_preset` / `film_curve` fail loudly.
        let bad_wb = call(3, "rune_edit_photo", json!({ "wb_preset": "mars_sun" }));
        assert_eq!(bad_wb["result"]["isError"], true);
        let bad_fc = call(
            4,
            "rune_edit_photo",
            json!({ "film_curve": "unknown_curve" }),
        );
        assert_eq!(bad_fc["result"]["isError"], true);

        // 4. `rune_edit_photo` with top-level `wb_preset` + `film_curve` + typed `patch` + `set`.
        let edit_resp = call(
            5,
            "rune_edit_photo",
            json!({
                "wb_preset": "tungsten",
                "film_curve": "film_extra_shadow",
                "patch": {
                    "clarity": 0.25,
                    "clarity_method": "punch",
                    "sharpen": 0.35,
                    "sharpen_radius": 1.6,
                    "sharpen_threshold": 0.12,
                    "sharpen_halo": 0.28,
                    "nr_color": 0.3,
                    "grain": 0.2,
                    "grain_type": "silver_rich",
                    "grain_size": 0.52,
                    "curves": { "rgb": [[0.0, 0.0], [0.25, 0.21], [0.75, 0.80], [1.0, 1.0]] },
                    "levels": { "red": { "in_black": 0.03 }, "blue": { "out_white": 0.95 } },
                    "wheels": { "master": { "hue_x": 0.12, "luma": 0.04 } },
                    "bw": { "enabled": true, "split_shadow_hue": 215.0, "split_shadow_sat": 0.24 }
                },
                "set": {
                    "levels.green.in_mid": 0.08,
                    "bw.split_highlight_hue": 45.0,
                    "bw.split_highlight_sat": 0.16,
                    "wheels.master_wheel.hue_y": -0.06
                }
            }),
        );
        assert_eq!(
            edit_resp["result"]["isError"], false,
            "phase1 mcp edit failed: {edit_resp}"
        );
        let rec = host.snap.lock().unwrap().recipe.clone();
        assert_eq!(rec.wb_preset, WbPreset::Tungsten);
        assert_eq!(rec.film_curve, FilmCurve::FilmExtraShadow);
        assert_eq!(rec.clarity_method, ClarityMethod::Punch);
        assert_eq!(rec.grain_type, GrainType::SilverRich);
        assert!((rec.sharpen_radius - 1.6).abs() < 1e-4);
        assert!((rec.sharpen_threshold - 0.12).abs() < 1e-4);
        assert!((rec.sharpen_halo - 0.28).abs() < 1e-4);
        assert!((rec.nr_color - 0.3).abs() < 1e-4);
        assert!((rec.grain_size - 0.52).abs() < 1e-4);
        assert_eq!(rec.curves.rgb.len(), 4);
        assert!((rec.levels.red.in_black - 0.03).abs() < 1e-4);
        assert!((rec.levels.green.in_mid - 0.08).abs() < 1e-4);
        assert!((rec.levels.blue.out_white - 0.95).abs() < 1e-4);
        assert!((rec.master_wheel.hue_x - 0.12).abs() < 1e-4);
        assert!((rec.master_wheel.hue_y - (-0.06)).abs() < 1e-4);
        assert!(rec.bw.enabled);
        assert!((rec.bw.split_shadow_hue - 215.0).abs() < 1e-3);
        assert!((rec.bw.split_highlight_sat - 0.16).abs() < 1e-4);

        // 5. `rune_inspect_photo` reflects updated Kelvin readout after tungsten WB preset.
        let insp1 = call(6, "rune_inspect_photo", json!({}));
        let k1 = insp1["result"]["structuredContent"]["wb_kelvin"]
            .as_u64()
            .expect("wb_kelvin");
        assert!(k1 < 4500, "expected tungsten Kelvin < 4500, got {k1}");
        assert_eq!(
            insp1["result"]["structuredContent"]["wb_preset"],
            "tungsten"
        );
        assert_eq!(
            insp1["result"]["structuredContent"]["film_curve"],
            "film_extra_shadow"
        );

        // 6. `rune_diff_photo` & `rune_preview_photo` succeed cleanly with Phase 1 edits active.
        let diff_resp = call(7, "rune_diff_photo", json!({}));
        assert_eq!(diff_resp["result"]["isError"], false);
        assert!(
            diff_resp["result"]["structuredContent"]["diff"]["delta_e2000"]["mean"]
                .as_f64()
                .unwrap_or(0.0)
                > 0.5
        );

        let prev_resp = call(
            8,
            "rune_preview_photo",
            json!({ "max_edge": 256, "quality": 80 }),
        );
        assert_eq!(prev_resp["result"]["isError"], false);
        assert!(
            prev_resp["result"]["structuredContent"]["jpeg_bytes"]
                .as_u64()
                .unwrap_or(0)
                > 100
        );
    }

    #[test]
    fn mcp_phase2_advanced_color_skin_lens_keystone_end_to_end() {
        use rune_editor_core::{CullFlag, DevelopRecipe, VignetteMethod};
        use rune_editor_decode::generate_demo_hdr_scene;
        use std::sync::Mutex;

        struct LiveHost {
            snap: Mutex<ActiveCanvasSnapshot>,
        }
        impl McpHostBridge for LiveHost {
            fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
                Some(self.snap.lock().unwrap().clone())
            }
            fn apply_live_recipe(
                &self,
                _target: Option<&std::path::Path>,
                new_recipe: DevelopRecipe,
                _summary: String,
            ) -> Result<bool, String> {
                self.snap.lock().unwrap().recipe = new_recipe;
                Ok(true)
            }
        }

        let demo = Arc::new(generate_demo_hdr_scene(120, 80));
        let host = Arc::new(LiveHost {
            snap: Mutex::new(ActiveCanvasSnapshot {
                display_name: "phase2.dng".to_owned(),
                source_path: None,
                image: demo,
                recipe: DevelopRecipe::default(),
                rating: 0,
                flag: CullFlag::None,
                color_tag: rune_editor_core::ColorTag::None,
                demo_mode: true,
                folder: None,
                catalog_len: 1,
            }),
        });
        let svc = McpService::new(host.clone());

        let call = |id: u64, name: &str, arguments: Value| {
            svc.handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(id)),
                method: "tools/call".to_owned(),
                params: Some(json!({ "name": name, "arguments": arguments })),
            })
            .unwrap()
        };

        // 1. `rune_get_schema` exposes every Phase 2 parameter ID.
        let schema_resp = call(1, "rune_get_schema", json!({}));
        assert_eq!(schema_resp["result"]["isError"], false);
        let params = schema_resp["result"]["structuredContent"]["schema"]["parameters"]
            .as_array()
            .expect("schema parameters array");
        for expected in [
            "advanced_color.0.enabled",
            "advanced_color.0.target_hue",
            "advanced_color.0.uniformity",
            "advanced_color.3.lightness",
            "skin_tone.enabled",
            "skin_tone.target_hue",
            "skin_tone.uniformity_hue",
            "skin_tone.amount_sat",
            "vignette_method",
            "lens.distortion",
            "lens.chromatic_aberration",
            "lens.purple_fringing",
            "lens.light_falloff",
            "geometry.keystone_vertical",
            "geometry.keystone_horizontal",
            "geometry.keystone_aspect",
            "geometry.keystone_amount",
        ] {
            assert!(
                params.iter().any(|p| p["id"] == expected),
                "rune_get_schema missing Phase 2 parameter '{expected}'"
            );
        }

        // 2. Strict validation rejects bad `skin_pick`, `color_slice_pick`, `vignette_method`.
        assert_eq!(
            call(2, "rune_edit_photo", json!({ "skin_pick": [1.5, 0.5] }))["result"]["isError"],
            true
        );
        assert_eq!(
            call(
                3,
                "rune_edit_photo",
                json!({ "color_slice_pick": { "index": 4, "x": 0.5, "y": 0.5 } })
            )["result"]["isError"],
            true
        );
        assert_eq!(
            call(
                4,
                "rune_edit_photo",
                json!({ "color_slice_pick": { "index": 0, "x": 0.5, "y": 0.5, "extra": 1 } })
            )["result"]["isError"],
            true
        );
        assert_eq!(
            call(5, "rune_edit_photo", json!({ "vignette_method": "square" }))["result"]["isError"],
            true
        );

        // 3. `rune_edit_photo` with top-level `skin_pick`, `color_slice_pick`, `vignette_method`,
        //    plus `patch` (refining picks & overriding vignette_method) + `set`.
        let edit_resp = call(
            6,
            "rune_edit_photo",
            json!({
                "skin_pick": [0.55, 0.45],
                "color_slice_pick": { "index": 0, "x": 0.25, "y": 0.20 },
                "vignette_method": "circular",
                "patch": {
                    "vignette": -0.30,
                    "vignette_method": "circular_on_crop",
                    "skin_tone": {
                        "uniformity_hue": 0.5,
                        "uniformity_sat": 0.3,
                        "amount_hue": 0.08
                    },
                    "advanced_color": {
                        "0": { "hue_range": 40.0, "uniformity": 0.45, "saturation": 0.22 }
                    },
                    "lens": {
                        "distortion": -0.18,
                        "chromatic_aberration": 0.25,
                        "purple_fringing": 0.40,
                        "light_falloff": 0.20
                    },
                    "geometry": {
                        "keystone_vertical": 0.24,
                        "keystone_horizontal": -0.12,
                        "keystone_aspect": 0.10,
                        "keystone_amount": 0.85
                    }
                },
                "set": {
                    "advanced_color.1.enabled": true,
                    "advanced_color.1.target_hue": 210.0,
                    "lens.light_falloff": 0.28,
                    "geometry.keystone_amount": 0.90
                }
            }),
        );
        assert_eq!(
            edit_resp["result"]["isError"], false,
            "phase2 mcp edit failed: {edit_resp}"
        );

        let rec = host.snap.lock().unwrap().recipe.clone();
        assert!(rec.skin_tone.enabled);
        assert!((rec.skin_tone.uniformity_hue - 0.5).abs() < 1e-4);
        assert!((rec.skin_tone.uniformity_sat - 0.3).abs() < 1e-4);
        assert!((rec.skin_tone.amount_hue - 0.08).abs() < 1e-4);
        assert!(rec.advanced_color[0].enabled);
        assert!((rec.advanced_color[0].hue_range - 40.0).abs() < 1e-3);
        assert!((rec.advanced_color[0].uniformity - 0.45).abs() < 1e-4);
        assert!(rec.advanced_color[1].enabled);
        assert!((rec.advanced_color[1].target_hue - 210.0).abs() < 1e-3);
        assert_eq!(rec.vignette_method, VignetteMethod::CircularOnCrop);
        assert!((rec.lens.distortion - (-0.18)).abs() < 1e-4);
        assert!((rec.lens.chromatic_aberration - 0.25).abs() < 1e-4);
        assert!((rec.lens.purple_fringing - 0.40).abs() < 1e-4);
        assert!((rec.lens.light_falloff - 0.28).abs() < 1e-4);
        assert!((rec.geometry.keystone_vertical - 0.24).abs() < 1e-4);
        assert!((rec.geometry.keystone_horizontal - (-0.12)).abs() < 1e-4);
        assert!((rec.geometry.keystone_aspect - 0.10).abs() < 1e-4);
        assert!((rec.geometry.keystone_amount - 0.90).abs() < 1e-4);

        // 4. `rune_inspect_photo`, `rune_diff_photo`, and `rune_preview_photo` all work cleanly.
        let insp = call(7, "rune_inspect_photo", json!({}));
        assert_eq!(insp["result"]["isError"], false);
        let overrides = insp["result"]["structuredContent"]["recipe_overrides"]
            .as_object()
            .expect("overrides");
        assert_eq!(overrides["vignette_method"], "circular_on_crop");
        assert!(overrides["lens"]["distortion"].as_f64().is_some());
        assert!(
            overrides["geometry"]["keystone_vertical"]
                .as_f64()
                .is_some()
        );
        assert_eq!(overrides["skin_tone"]["enabled"], true);
        assert_eq!(overrides["advanced_color"]["0"]["enabled"], true);

        let diff_resp = call(8, "rune_diff_photo", json!({}));
        assert_eq!(diff_resp["result"]["isError"], false);
        assert!(
            diff_resp["result"]["structuredContent"]["diff"]["delta_e2000"]["mean"]
                .as_f64()
                .unwrap_or(0.0)
                > 0.1
        );

        let prev_resp = call(
            9,
            "rune_preview_photo",
            json!({ "max_edge": 256, "quality": 80 }),
        );
        assert_eq!(prev_resp["result"]["isError"], false);
        assert!(
            prev_resp["result"]["structuredContent"]["jpeg_bytes"]
                .as_u64()
                .unwrap_or(0)
                > 100
        );
    }

    #[test]
    fn mcp_phase3_variants_styles_tags_clipboard_and_export_end_to_end() {
        use rune_editor_core::{ColorTag, DevelopRecipe, PhotoMeta};
        use rune_editor_decode::generate_demo_hdr_scene;

        let dir = unique_test_dir("phase3_workflow");
        let styles_dir = dir.join("styles");
        let export_dir = dir.join("exports");
        let photo_a = dir.join("frame_a.jpg");
        let photo_b = dir.join("frame_b.jpg");

        let demo = generate_demo_hdr_scene(80, 60);
        rune_editor_decode::export_image(
            &demo,
            &DevelopRecipe::default(),
            &photo_a,
            rune_editor_decode::ExportFormat::Jpeg { quality: 85 },
        )
        .expect("write photo_a");
        rune_editor_decode::export_image(
            &demo,
            &DevelopRecipe::default(),
            &photo_b,
            rune_editor_decode::ExportFormat::Jpeg { quality: 85 },
        )
        .expect("write photo_b");

        let svc = McpService::new(Arc::new(HeadlessHostBridge)).with_styles_dir(styles_dir.clone());
        let call = |id: u64, name: &str, arguments: Value| {
            svc.handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(id)),
                method: "tools/call".to_owned(),
                params: Some(json!({ "name": name, "arguments": arguments })),
            })
            .unwrap()
        };

        // 0. Security: untrusted `styles_dir` argument in MCP JSON-RPC MUST be rejected!
        let bad_dir = call(
            100,
            "rune_manage_workflow",
            json!({
                "action": "style_delete",
                "name": "anything",
                "styles_dir": "/tmp/victim",
            }),
        );
        assert_eq!(bad_dir["result"]["isError"], true);
        assert_eq!(
            bad_dir["result"]["structuredContent"]["code"],
            "UNKNOWN_ARGUMENT"
        );

        // 1. Cull with `color_tag` + inspect returns color_tag and variants summary.
        let cull = call(
            1,
            "rune_cull_or_export",
            json!({ "action": "cull", "path": photo_a, "rating": 5, "flag": "pick", "color_tag": "green" }),
        );
        assert_eq!(cull["result"]["isError"], false, "cull failed: {cull}");
        assert_eq!(cull["result"]["structuredContent"]["color_tag"], "green");

        let insp = call(2, "rune_inspect_photo", json!({ "path": photo_a }));
        assert_eq!(insp["result"]["structuredContent"]["color_tag"], "green");
        assert_eq!(
            insp["result"]["structuredContent"]["active_variant_index"],
            0
        );
        assert_eq!(insp["result"]["structuredContent"]["variant_count"], 1);
        assert_eq!(
            insp["result"]["structuredContent"]["variants"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        // 2. Virtual Variants via `rune_manage_workflow`.
        let _ = call(
            3,
            "rune_edit_photo",
            json!({ "path": photo_a, "patch": { "exposure": 0.4, "contrast": 0.2 } }),
        );
        let v_clone = call(
            4,
            "rune_manage_workflow",
            json!({ "action": "variant_clone", "path": photo_a, "label": "Warm Cut" }),
        );
        assert_eq!(v_clone["result"]["isError"], false);
        assert_eq!(v_clone["result"]["structuredContent"]["variant_count"], 2);

        let v_new = call(
            5,
            "rune_manage_workflow",
            json!({ "action": "variant_new", "path": photo_a, "label": "Clean Base" }),
        );
        assert_eq!(v_new["result"]["isError"], false);
        assert_eq!(v_new["result"]["structuredContent"]["variant_count"], 3);

        let v_sel = call(
            6,
            "rune_manage_workflow",
            json!({ "action": "variant_select", "path": photo_a, "index": 1 }),
        );
        assert_eq!(v_sel["result"]["isError"], false);
        assert_eq!(
            v_sel["result"]["structuredContent"]["active_variant_index"],
            1
        );

        let v_del = call(
            7,
            "rune_manage_workflow",
            json!({ "action": "variant_delete", "path": photo_a, "index": 2 }),
        );
        assert_eq!(v_del["result"]["isError"], false);
        assert_eq!(v_del["result"]["structuredContent"]["variant_count"], 2);

        // 3. User Styles security: path-traversal style names MUST fail loudly!
        for evil_name in ["../pwn", "sub/dir", ".hidden", ""] {
            let bad = call(
                8,
                "rune_manage_workflow",
                json!({
                    "action": "style_save",
                    "path": photo_a,
                    "name": evil_name,
                }),
            );
            assert_eq!(
                bad["result"]["isError"], true,
                "style_save must reject traversal name '{evil_name}'"
            );
        }

        // Save, list, stack-apply, and delete a valid User Style.
        let s_save = call(
            9,
            "rune_manage_workflow",
            json!({
                "action": "style_save",
                "path": photo_a,
                "name": "Studio Crisp",
                "autoselect_adjusted": true,
            }),
        );
        assert_eq!(s_save["result"]["isError"], false, "style_save: {s_save}");

        let s_list = call(
            10,
            "rune_manage_workflow",
            json!({ "action": "style_list" }),
        );
        assert_eq!(s_list["result"]["structuredContent"]["count"], 1);

        let _ = call(
            11,
            "rune_edit_photo",
            json!({ "path": photo_b, "patch": { "temperature": 0.25 } }),
        );
        let s_apply = call(
            12,
            "rune_manage_workflow",
            json!({
                "action": "style_apply",
                "path": photo_b,
                "name": "Studio Crisp",
                "stack": true,
                "strength": 0.5,
            }),
        );
        assert_eq!(
            s_apply["result"]["isError"], false,
            "style_apply: {s_apply}"
        );
        let meta_b = PhotoMeta::load_sidecar(&photo_b).unwrap().unwrap();
        assert!((meta_b.recipe.temperature - 0.25).abs() < 1e-4);
        assert!((meta_b.recipe.exposure - 0.2).abs() < 1e-4);

        let s_del = call(
            13,
            "rune_manage_workflow",
            json!({ "action": "style_delete", "name": "Studio Crisp" }),
        );
        assert_eq!(s_del["result"]["isError"], false);

        // 4. Selective Adjustments Clipboard (`copy_selective_groups`).
        let _ = call(
            14,
            "rune_edit_photo",
            json!({ "path": photo_a, "patch": { "tint": -0.15 } }),
        );
        let cp = call(
            15,
            "rune_manage_workflow",
            json!({
                "action": "copy_selective_groups",
                "src_path": photo_a,
                "dst_path": photo_b,
                "groups": ["wb"],
            }),
        );
        assert_eq!(cp["result"]["isError"], false, "copy_selective: {cp}");
        let meta_b2 = PhotoMeta::load_sidecar(&photo_b).unwrap().unwrap();
        assert!((meta_b2.recipe.tint - (-0.15)).abs() < 1e-4);
        assert!((meta_b2.recipe.exposure - 0.2).abs() < 1e-4);

        // 5. Multi-Recipe Export + Watermark + Display P3 + Naming Template + Security checks.
        let bad_sub = call(
            16,
            "rune_cull_or_export",
            json!({
                "action": "export",
                "path": photo_a,
                "out": export_dir,
                "recipes": [{ "name": "Evil", "enabled": true, "subfolder": "../escape" }],
            }),
        );
        assert_eq!(
            bad_sub["result"]["isError"], true,
            "multi-recipe subfolder traversal MUST be rejected"
        );

        let exp = call(
            17,
            "rune_cull_or_export",
            json!({
                "action": "export",
                "path": photo_a,
                "out": export_dir,
                "naming_template": "{ImageName}_{Recipe}_{Variant}",
                "watermark": { "text": "RUNE PRO", "opacity": 0.5, "position": "bottom_right" },
                "strip_metadata": true,
                "color_space": "display_p3",
                "recipes": ["full_res_jpeg", "web_2048px"],
            }),
        );
        assert_eq!(
            exp["result"]["isError"], false,
            "multi-recipe export: {exp}"
        );
        assert_eq!(
            exp["result"]["structuredContent"]["outputs"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        // Re-running without `overwrite: true` MUST fail with FILE_EXISTS!
        let exp_dup = call(
            18,
            "rune_cull_or_export",
            json!({
                "action": "export",
                "path": photo_a,
                "out": export_dir,
                "naming_template": "{ImageName}_{Recipe}_{Variant}",
                "recipes": ["full_res_jpeg", "web_2048px"],
            }),
        );
        assert_eq!(exp_dup["result"]["isError"], true);
        assert_eq!(
            exp_dup["result"]["structuredContent"]["code"],
            "FILE_EXISTS"
        );

        // Unknown top-level argument on `rune_cull_or_export` fails loudly!
        let bad_arg = call(
            19,
            "rune_cull_or_export",
            json!({ "action": "cull", "path": photo_a, "bogus": 123 }),
        );
        assert_eq!(bad_arg["result"]["isError"], true);
        assert_eq!(
            PhotoMeta::load_sidecar(&photo_a)
                .unwrap()
                .unwrap()
                .color_tag,
            ColorTag::Green
        );

        // 6. Live GUI canvas `color_tag` propagation via `McpHostBridge::apply_live_cull` & `rune_inspect_photo`.
        struct LiveCullHost {
            snap: std::sync::Mutex<ActiveCanvasSnapshot>,
        }
        impl McpHostBridge for LiveCullHost {
            fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
                Some(self.snap.lock().unwrap().clone())
            }
            fn apply_live_cull(
                &self,
                _target: Option<&std::path::Path>,
                rating: Option<u8>,
                flag: Option<rune_editor_core::CullFlag>,
                color_tag: Option<ColorTag>,
            ) -> Result<bool, String> {
                let mut g = self.snap.lock().unwrap();
                if let Some(r) = rating {
                    g.rating = r;
                }
                if let Some(f) = flag {
                    g.flag = f;
                }
                if let Some(t) = color_tag {
                    g.color_tag = t;
                }
                Ok(true)
            }
        }
        let live_host = Arc::new(LiveCullHost {
            snap: std::sync::Mutex::new(ActiveCanvasSnapshot {
                display_name: "live.dng".to_owned(),
                source_path: None,
                image: Arc::new(demo),
                recipe: DevelopRecipe::default(),
                rating: 0,
                flag: rune_editor_core::CullFlag::None,
                color_tag: ColorTag::None,
                demo_mode: true,
                folder: None,
                catalog_len: 1,
            }),
        });
        let live_svc = McpService::new(live_host.clone());
        let live_cull = live_svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(20)),
                method: "tools/call".to_owned(),
                params: Some(json!({
                    "name": "rune_cull_or_export",
                    "arguments": { "action": "cull", "color_tag": "red", "rating": 4 }
                })),
            })
            .unwrap();
        assert_eq!(live_cull["result"]["isError"], false);
        assert_eq!(
            live_cull["result"]["structuredContent"]["live_gui_updated"],
            true
        );
        assert_eq!(live_host.snap.lock().unwrap().color_tag, ColorTag::Red);

        let live_insp = live_svc
            .handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(21)),
                method: "tools/call".to_owned(),
                params: Some(json!({ "name": "rune_inspect_photo", "arguments": {} })),
            })
            .unwrap();
        assert_eq!(live_insp["result"]["structuredContent"]["color_tag"], "red");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_phase4_layers_masks_ai_and_heal_end_to_end() {
        use rune_editor_core::{ColorTag, CullFlag, DevelopRecipe, HealMode, MaskCombineOp};
        use rune_editor_decode::generate_demo_hdr_scene;
        use std::sync::Mutex;

        struct LiveLayerHost {
            snap: Mutex<ActiveCanvasSnapshot>,
        }
        impl McpHostBridge for LiveLayerHost {
            fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
                Some(self.snap.lock().unwrap().clone())
            }
            fn apply_live_recipe(
                &self,
                _target: Option<&std::path::Path>,
                new_recipe: DevelopRecipe,
                _summary: String,
            ) -> Result<bool, String> {
                self.snap.lock().unwrap().recipe = new_recipe;
                Ok(true)
            }
        }

        let demo = Arc::new(generate_demo_hdr_scene(120, 80));
        let host = Arc::new(LiveLayerHost {
            snap: Mutex::new(ActiveCanvasSnapshot {
                display_name: "phase4.dng".to_owned(),
                source_path: None,
                image: demo,
                recipe: DevelopRecipe::default(),
                rating: 0,
                flag: CullFlag::None,
                color_tag: ColorTag::None,
                demo_mode: true,
                folder: None,
                catalog_len: 1,
            }),
        });
        let svc = McpService::new(host.clone());
        let call = |id: u64, name: &str, arguments: Value| {
            svc.handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(id)),
                method: "tools/call".to_owned(),
                params: Some(json!({ "name": name, "arguments": arguments })),
            })
            .unwrap()
        };

        // 1. Strict validation: unknown arg, out-of-range opacity/target, unknown local key fail loudly.
        assert_eq!(
            call(
                1,
                "rune_edit_layers",
                json!({ "action": "layer_list", "bogus": true })
            )["result"]["structuredContent"]["code"],
            "UNKNOWN_ARGUMENT"
        );
        assert_eq!(
            call(
                2,
                "rune_edit_layers",
                json!({ "action": "layer_add", "name": "Bad", "opacity": 1.5 })
            )["result"]["isError"],
            true
        );
        assert_eq!(
            call(
                3,
                "rune_edit_layers",
                json!({ "action": "heal_add", "target": [1.5, 0.5] })
            )["result"]["isError"],
            true
        );
        // Radial radius must be within 0.01..=1.5, luma requires low <= high, brush <= 1024 dabs.
        assert_eq!(
            call(
                301,
                "rune_edit_layers",
                json!({ "action": "layer_add", "name": "BadRad", "kind": "radial", "radius": 1.8 })
            )["result"]["structuredContent"]["code"],
            "VALIDATION_ERROR"
        );
        assert_eq!(
            call(
                302,
                "rune_edit_layers",
                json!({ "action": "layer_add", "name": "BadLuma", "kind": "luma", "low": 0.8, "high": 0.3 })
            )["result"]["structuredContent"]["code"],
            "VALIDATION_ERROR"
        );
        let huge_strokes: Vec<Value> = (0..1025)
            .map(|_| json!({ "x": 0.5, "y": 0.5, "radius": 0.02, "hardness": 0.5, "opacity": 0.5, "erase": false }))
            .collect();
        assert_eq!(
            call(
                303,
                "rune_edit_layers",
                json!({ "action": "layer_add", "name": "DoSBrush", "kind": "brush", "strokes": huge_strokes })
            )["result"]["structuredContent"]["code"],
            "VALIDATION_ERROR"
        );
        // `layer_add` honors `enabled: false`.
        let disabled_add = call(
            304,
            "rune_edit_layers",
            json!({ "action": "layer_add", "name": "Muted", "kind": "filled", "enabled": false }),
        );
        assert_eq!(disabled_add["result"]["isError"], false);
        assert!(!host.snap.lock().unwrap().recipe.layers[0].enabled);
        assert_eq!(
            call(
                305,
                "rune_edit_layers",
                json!({ "action": "layer_remove", "index": 0 })
            )["result"]["structuredContent"]["layer_count"],
            0
        );
        // `rune_manage_workflow` schema documents `layers` & `heal` groups.
        let wf_tool = tools::tool_definitions()
            .into_iter()
            .find(|t| t.name == "rune_manage_workflow")
            .expect("rune_manage_workflow tool");
        let groups_desc = wf_tool.input_schema["properties"]["groups"]["description"]
            .as_str()
            .unwrap_or_default();
        assert!(groups_desc.contains("layers") && groups_desc.contains("heal"));

        // 2. Add linear gradient layer + subtract radial + intersect luma masks.
        let l_add = call(
            4,
            "rune_edit_layers",
            json!({
                "action": "layer_add",
                "name": "Sky Burn",
                "kind": "linear",
                "opacity": 0.85,
                "start": [0.5, 0.0],
                "end": [0.5, 0.55],
                "adjustments": { "exposure": -0.6, "dehaze": 0.25 }
            }),
        );
        assert_eq!(l_add["result"]["isError"], false, "layer_add: {l_add}");
        assert_eq!(
            l_add["result"]["structuredContent"]["live_gui_updated"],
            true
        );

        let m_sub = call(
            5,
            "rune_edit_layers",
            json!({
                "action": "mask_add",
                "index": 0,
                "kind": "radial",
                "op": "subtract",
                "center": [0.5, 0.5],
                "radius": [0.25, 0.2],
                "feather": 0.45
            }),
        );
        assert_eq!(
            m_sub["result"]["isError"], false,
            "mask_add radial: {m_sub}"
        );

        let m_int = call(
            6,
            "rune_edit_layers",
            json!({
                "action": "mask_add",
                "index": 0,
                "kind": "luma",
                "op": "intersect",
                "low": 0.45,
                "high": 1.0,
                "low_falloff": 0.12
            }),
        );
        assert_eq!(m_int["result"]["isError"], false, "mask_add luma: {m_int}");

        // 3. AI Subject & AI Background layers.
        let ai_sub = call(
            7,
            "rune_edit_layers",
            json!({
                "action": "ai_subject",
                "adjustments": { "exposure": 0.35, "clarity": 0.2 }
            }),
        );
        assert_eq!(ai_sub["result"]["isError"], false, "ai_subject: {ai_sub}");

        let ai_bg = call(
            8,
            "rune_edit_layers",
            json!({
                "action": "ai_background",
                "opacity": 0.75,
                "adjustments": { "exposure": -0.4 }
            }),
        );
        assert_eq!(ai_bg["result"]["isError"], false, "ai_bg: {ai_bg}");

        // 4. Update layer #0 & list layers.
        let l_upd = call(
            9,
            "rune_edit_layers",
            json!({
                "action": "layer_update",
                "index": 0,
                "opacity": 0.9,
                "adjustments": { "contrast": 0.15 }
            }),
        );
        assert_eq!(l_upd["result"]["isError"], false);

        let l_list = call(10, "rune_edit_layers", json!({ "action": "layer_list" }));
        assert_eq!(l_list["result"]["structuredContent"]["layer_count"], 3);

        // 5. Add Spot Heal (auto-source) + Clone stamp + list spots.
        let h1 = call(
            11,
            "rune_edit_layers",
            json!({ "action": "heal_add", "target": [0.42, 0.38], "radius": 0.03, "mode": "heal" }),
        );
        assert_eq!(h1["result"]["isError"], false, "heal_add: {h1}");
        let h2 = call(
            12,
            "rune_edit_layers",
            json!({
                "action": "heal_add",
                "target": [0.70, 0.60],
                "source": [0.60, 0.60],
                "radius": 0.04,
                "mode": "clone"
            }),
        );
        assert_eq!(h2["result"]["isError"], false, "clone_add: {h2}");

        let h_list = call(13, "rune_edit_layers", json!({ "action": "heal_list" }));
        assert_eq!(h_list["result"]["structuredContent"]["heal_spot_count"], 2);

        // 6. Inspect + live preview rendering with combined masks, AI Subject & Heal/Clone active!
        let insp = call(14, "rune_inspect_photo", json!({}));
        assert_eq!(insp["result"]["structuredContent"]["layer_count"], 3);
        assert_eq!(insp["result"]["structuredContent"]["heal_spot_count"], 2);

        let prev = call(
            15,
            "rune_preview_photo",
            json!({ "max_edge": 256, "quality": 80 }),
        );
        assert_eq!(prev["result"]["isError"], false, "preview failed: {prev}");
        assert!(
            prev["result"]["structuredContent"]["jpeg_bytes"]
                .as_u64()
                .unwrap_or(0)
                > 100
        );

        let rec = host.snap.lock().unwrap().recipe.clone();
        assert_eq!(rec.layers[0].masks[1].op, MaskCombineOp::Subtract);
        assert_eq!(rec.layers[0].masks[2].op, MaskCombineOp::Intersect);
        assert_eq!(rec.heal_spots[0].mode, HealMode::Heal);
        assert_eq!(rec.heal_spots[1].mode, HealMode::Clone);

        // 7. Mask clear, layer remove, heal remove & clear.
        assert_eq!(
            call(
                16,
                "rune_edit_layers",
                json!({ "action": "mask_clear", "index": 0 })
            )["result"]["isError"],
            false
        );
        assert_eq!(
            call(
                17,
                "rune_edit_layers",
                json!({ "action": "layer_remove", "index": 2 })
            )["result"]["structuredContent"]["layer_count"],
            2
        );
        assert_eq!(
            call(
                18,
                "rune_edit_layers",
                json!({ "action": "heal_remove", "index": 0 })
            )["result"]["structuredContent"]["heal_spot_count"],
            1
        );
        assert_eq!(
            call(19, "rune_edit_layers", json!({ "action": "heal_clear" }))["result"]["structuredContent"]
                ["heal_spot_count"],
            0
        );
    }

    #[test]
    fn mcp_phase5_film_negative_and_session_init_end_to_end() {
        use std::sync::Mutex;

        use rune_editor_core::{ColorTag, CullFlag, DevelopRecipe, ProcessingMode};
        use rune_editor_decode::generate_demo_hdr_scene;

        struct LivePhase5Host {
            snap: Mutex<ActiveCanvasSnapshot>,
        }
        impl McpHostBridge for LivePhase5Host {
            fn active_canvas(&self) -> Option<ActiveCanvasSnapshot> {
                Some(self.snap.lock().unwrap().clone())
            }
            fn apply_live_recipe(
                &self,
                _target: Option<&std::path::Path>,
                new_recipe: DevelopRecipe,
                _summary: String,
            ) -> Result<bool, String> {
                self.snap.lock().unwrap().recipe = new_recipe;
                Ok(true)
            }
        }

        let dir = unique_test_dir("phase5_film_neg_mcp");
        let photo = dir.join("scan_01.jpg");
        let demo = Arc::new(generate_demo_hdr_scene(80, 60));
        rune_editor_decode::export_image(
            &demo,
            &DevelopRecipe::default(),
            &photo,
            rune_editor_decode::ExportFormat::Jpeg { quality: 85 },
        )
        .expect("write photo");

        let host = Arc::new(LivePhase5Host {
            snap: Mutex::new(ActiveCanvasSnapshot {
                display_name: "scan_01.jpg".to_owned(),
                source_path: Some(photo.clone()),
                image: demo,
                recipe: DevelopRecipe::default(),
                rating: 0,
                flag: CullFlag::None,
                color_tag: ColorTag::None,
                demo_mode: false,
                folder: Some(dir.clone()),
                catalog_len: 1,
            }),
        });
        let svc = McpService::new(host.clone());
        let call = |id: u64, name: &str, arguments: Value| {
            svc.handle_request(&protocol::JsonRpcRequest {
                jsonrpc: "2.0".to_owned(),
                id: Some(json!(id)),
                method: "tools/call".to_owned(),
                params: Some(json!({ "name": name, "arguments": arguments })),
            })
            .unwrap()
        };

        // 1. Strict validation on `processing_mode`, `film_base_pick`, and `session_init`.
        assert_eq!(
            call(
                1,
                "rune_edit_photo",
                json!({ "processing_mode": "slides_unknown" })
            )["result"]["structuredContent"]["code"],
            "VALIDATION_ERROR"
        );
        assert_eq!(
            call(
                2,
                "rune_edit_photo",
                json!({ "film_base_pick": [1.5, 0.5] })
            )["result"]["structuredContent"]["code"],
            "VALIDATION_ERROR"
        );
        assert_eq!(
            call(
                3,
                "rune_manage_workflow",
                json!({ "action": "session_init", "dir": "/tmp/../escape" })
            )["result"]["structuredContent"]["code"],
            "VALIDATION_ERROR"
        );
        assert_eq!(
            call(
                4,
                "rune_manage_workflow",
                json!({ "action": "session_init", "dir": photo })
            )["result"]["structuredContent"]["code"],
            "VALIDATION_ERROR"
        );
        assert_eq!(
            call(
                5,
                "rune_manage_workflow",
                json!({ "action": "style_list", "dir": dir })
            )["result"]["structuredContent"]["code"],
            "VALIDATION_ERROR"
        );

        // 2. Edit with `processing_mode` + `film_base_pick` BEFORE `patch` & `set`.
        let edit = call(
            6,
            "rune_edit_photo",
            json!({
                "processing_mode": "film_negative",
                "film_base_pick": [0.5, 0.5],
                "patch": { "film_negative": { "density": 0.30, "black_clip": 0.03 } },
                "set": { "film_negative.contrast": 0.22 }
            }),
        );
        assert_eq!(edit["result"]["isError"], false, "phase5 edit: {edit}");
        assert_eq!(
            edit["result"]["structuredContent"]["processing_mode"],
            "film_negative"
        );
        let rec = host.snap.lock().unwrap().recipe.clone();
        assert_eq!(rec.processing_mode, ProcessingMode::FilmNegative);
        assert!((rec.film_negative.density - 0.30).abs() < 1e-4);
        assert!((rec.film_negative.contrast - 0.22).abs() < 1e-4);
        assert!((rec.film_negative.black_clip - 0.03).abs() < 1e-4);

        // 3. `rune_inspect_photo` exposes `processing_mode` & `film_negative` on both live canvas & explicit path.
        let insp_live = call(7, "rune_inspect_photo", json!({}));
        assert_eq!(insp_live["result"]["isError"], false);
        assert_eq!(
            insp_live["result"]["structuredContent"]["processing_mode"],
            "film_negative"
        );
        assert!(
            (insp_live["result"]["structuredContent"]["film_negative"]["density"]
                .as_f64()
                .unwrap()
                - 0.30)
                .abs()
                < 1e-4
        );
        let insp_disk = call(8, "rune_inspect_photo", json!({ "path": photo }));
        assert_eq!(
            insp_disk["result"]["structuredContent"]["processing_mode"],
            "film_negative"
        );
        assert!(
            (insp_disk["result"]["structuredContent"]["film_negative"]["contrast"]
                .as_f64()
                .unwrap()
                - 0.22)
                .abs()
                < 1e-4
        );

        // 4. `rune_preview_photo` renders Film Negative cleanly.
        let prev = call(
            9,
            "rune_preview_photo",
            json!({ "max_edge": 256, "quality": 80 }),
        );
        assert_eq!(prev["result"]["isError"], false);
        assert!(
            prev["result"]["structuredContent"]["jpeg_bytes"]
                .as_u64()
                .unwrap_or(0)
                > 100
        );

        // 5. `rune_manage_workflow` `session_init` creates all 4 Capture One folders cleanly.
        let sess_dir = dir.join("mcp_session");
        let sess = call(
            10,
            "rune_manage_workflow",
            json!({ "action": "session_init", "dir": sess_dir }),
        );
        assert_eq!(sess["result"]["isError"], false, "session_init: {sess}");
        assert!(sess_dir.join("Capture").is_dir());
        assert!(sess_dir.join("Selects").is_dir());
        assert!(sess_dir.join("Output").is_dir());
        assert!(sess_dir.join("Trash").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
