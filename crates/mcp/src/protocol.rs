//! Model Context Protocol (`2025-11-25`) JSON-RPC 2.0 types, version negotiation,
//! tool annotations, structured outputs, and RFC 4648 base64 encoding.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Latest supported MCP specification revision.
pub const MCP_PROTOCOL_LATEST: &str = "2025-11-25";

/// Supported MCP revisions ordered newest-first for handshake negotiation.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Returns `true` when `version` is one of [`SUPPORTED_PROTOCOL_VERSIONS`].
pub fn is_supported_protocol_version(version: &str) -> bool {
    let trimmed = version.trim();
    SUPPORTED_PROTOCOL_VERSIONS.contains(&trimmed)
}

/// Negotiates the MCP protocol version from a client's `initialize` request.
pub fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    if let Some(req) = requested {
        for &supported in SUPPORTED_PROTOCOL_VERSIONS {
            if supported == req {
                return supported;
            }
        }
    }
    MCP_PROTOCOL_LATEST
}

#[derive(Clone, Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    pub fn is_valid_jsonrpc(&self) -> bool {
        self.jsonrpc == "2.0"
    }
}

pub fn jsonrpc_ok(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

pub fn jsonrpc_error(id: Option<Value>, code: i32, message: impl Into<String>) -> Value {
    let mut obj = json!({
        "jsonrpc": "2.0",
        "error": {
            "code": code,
            "message": message.into(),
        }
    });
    if let Some(req_id) = id {
        obj["id"] = req_id;
    } else {
        obj["id"] = Value::Null;
    }
    obj
}

/// Standard JSON-RPC 2.0 error codes.
pub mod rpc_err {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// MCP `ToolAnnotations` behavioral hints (2025-03-26 / 2025-11-25).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
    pub open_world_hint: bool,
}

/// MCP `Tool` descriptor with `title`, `inputSchema`, `outputSchema`, and `annotations`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub output_schema: Value,
    pub annotations: ToolAnnotations,
}

/// Builds a compliant `CallToolResult` containing BOTH `structuredContent` AND
/// backward-compatible `TextContent` (plus optional inline `ImageContent`).
pub fn tool_ok(
    structured: Value,
    summary_text: Option<String>,
    image_jpeg: Option<&[u8]>,
) -> Value {
    let text = summary_text
        .unwrap_or_else(|| serde_json::to_string(&structured).unwrap_or_else(|_| "{}".to_owned()));
    let mut content = vec![json!({
        "type": "text",
        "text": text,
    })];
    if let Some(jpeg_bytes) = image_jpeg {
        content.push(json!({
            "type": "image",
            "data": encode_base64(jpeg_bytes),
            "mimeType": "image/jpeg",
        }));
    }
    json!({
        "content": content,
        "structuredContent": structured,
        "isError": false,
    })
}

/// Builds an MCP tool-level error (`isError: true`) so the agent can self-correct.
pub fn tool_err(code: &str, message: impl Into<String>, hint: Option<&str>) -> Value {
    let msg = message.into();
    let structured = json!({
        "ok": false,
        "code": code,
        "message": msg,
        "hint": hint,
    });
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string(&structured).unwrap_or(msg),
        }],
        "structuredContent": structured,
        "isError": true,
    })
}

/// Zero-dependency RFC 4648 standard Base64 encoder for inline MCP `ImageContent`.
pub fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push(TABLE[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn protocol_version_negotiates_cleanly() {
        assert_eq!(negotiate_protocol_version(Some("2025-11-25")), "2025-11-25");
        assert_eq!(negotiate_protocol_version(Some("2025-03-26")), "2025-03-26");
        assert_eq!(
            negotiate_protocol_version(Some("2099-01-01")),
            MCP_PROTOCOL_LATEST
        );
        assert_eq!(negotiate_protocol_version(None), MCP_PROTOCOL_LATEST);
        assert!(is_supported_protocol_version("2025-11-25"));
        assert!(is_supported_protocol_version("2024-11-05"));
        assert!(!is_supported_protocol_version("1999-01-01"));
        assert!(!is_supported_protocol_version(""));
    }
}
