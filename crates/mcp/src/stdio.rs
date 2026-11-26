//! Stdio MCP `2025-11-25` transport (`rd mcp`) — newline-delimited JSON-RPC 2.0
//! over `stdin` / `stdout` (zero non-JSON output on stdout).

use std::io::{self, BufRead, Read, Write};

use crate::protocol::{JsonRpcRequest, jsonrpc_error, rpc_err};
use crate::tools::McpService;

/// Maximum byte length of a single newline-delimited JSON-RPC message on stdio (`8 MiB`).
pub const MAX_STDIO_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Runs a blocking stdio MCP JSON-RPC 2.0 loop until EOF on `reader`.
pub fn serve_stdio<R: BufRead, W: Write>(
    service: &McpService,
    mut reader: R,
    mut writer: W,
) -> io::Result<()> {
    let mut buf = Vec::new();
    let max_read = (MAX_STDIO_LINE_BYTES as u64).saturating_add(1);

    loop {
        buf.clear();
        let n = (&mut reader).take(max_read).read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        if buf.len() > MAX_STDIO_LINE_BYTES {
            if !buf.ends_with(b"\n") {
                drain_until_newline(&mut reader)?;
            }
            let payload = jsonrpc_error(
                None,
                rpc_err::INVALID_REQUEST,
                format!(
                    "JSON-RPC message exceeds {} MiB limit",
                    MAX_STDIO_LINE_BYTES / (1024 * 1024)
                ),
            );
            let encoded = serde_json::to_string(&payload).unwrap_or_default();
            writeln!(writer, "{encoded}")?;
            writer.flush()?;
            continue;
        }

        let line = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(e) => {
                let payload = jsonrpc_error(
                    None,
                    rpc_err::PARSE_ERROR,
                    format!("Invalid UTF-8 in JSON-RPC message: {e}"),
                );
                let encoded = serde_json::to_string(&payload).unwrap_or_default();
                writeln!(writer, "{encoded}")?;
                writer.flush()?;
                continue;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(req) => service.handle_request(&req),
            Err(e) => Some(jsonrpc_error(
                None,
                rpc_err::PARSE_ERROR,
                format!("Invalid JSON-RPC message: {e}"),
            )),
        };
        if let Some(payload) = resp {
            let encoded = serde_json::to_string(&payload).unwrap_or_default();
            writeln!(writer, "{encoded}")?;
            writer.flush()?;
        }
    }
    Ok(())
}

/// Drains remaining bytes of an oversized line until `\n` or EOF without growing memory.
fn drain_until_newline<R: BufRead>(reader: &mut R) -> io::Result<()> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(());
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            reader.consume(pos + 1);
            return Ok(());
        }
        let len = available.len();
        reader.consume(len);
    }
}
