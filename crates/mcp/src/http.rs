//! Localhost-only MCP `2025-11-25` Streamable HTTP Server (`127.0.0.1:<port>/mcp`).
//!
//! Security & specification compliance:
//! - Binds strictly to `127.0.0.1` (never `0.0.0.0`).
//! - Requires `Host` on HTTP/1.1 (`400 Bad Request` if absent) and strictly validates both `Host`
//!   and `Origin` against localhost with numeric `u16` ports (`403 Forbidden` otherwise).
//! - Bounds request/header/chunk-size/trailer lines (`8 KiB`), header count (`64`), trailer count
//!   (`16`), body size (`4 MiB`), and concurrent worker threads (`16`, returning `503` when full).
//! - Rejects HTTP/1.1 framing ambiguities (`Transfer-Encoding` + `Content-Length`, duplicate
//!   `Content-Length`, invalid chunk CRLF) with `400 Bad Request`.
//! - Issues CSPRNG visible-ASCII `Mcp-Session-Id` on `initialize` in a bounded LRU (`256`),
//!   honors `DELETE /mcp`, and always returns `404 Not Found` for unknown or terminated sessions.
//! - Validates `MCP-Protocol-Version` on `POST` (`400` if unsupported) and `jsonrpc == "2.0"`.
//! - Returns `202 Accepted` on JSON-RPC notifications and `405 Method Not Allowed` (`Allow: POST, DELETE, OPTIONS`) on `GET /mcp`.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::protocol::{
    JsonRpcRequest, MCP_PROTOCOL_LATEST, SUPPORTED_PROTOCOL_VERSIONS,
    is_supported_protocol_version, jsonrpc_error, rpc_err,
};
use crate::tools::McpService;

const MAX_HTTP_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_HTTP_LINE_BYTES: usize = 8 * 1024;
const MAX_HTTP_HEADERS: usize = 64;
const MAX_HTTP_TRAILERS: usize = 16;
const MAX_CONCURRENT_HTTP_CONNS: usize = 16;
const MAX_MCP_SESSIONS: usize = 256;
const IDLE_READ_TIMEOUT: Duration = Duration::from_secs(10);
const ACTIVE_IO_TIMEOUT: Duration = Duration::from_secs(60);

static SESSION_SEQ: AtomicU64 = AtomicU64::new(1);

/// Bounded LRU store of active MCP session IDs that prevents post-`DELETE` reuse bypass.
struct SessionStore {
    active: VecDeque<String>,
    sessions_issued: bool,
}

impl SessionStore {
    fn new() -> Self {
        Self {
            active: VecDeque::with_capacity(MAX_MCP_SESSIONS),
            sessions_issued: false,
        }
    }

    fn issue(&mut self, sid: String) {
        self.sessions_issued = true;
        if let Some(pos) = self.active.iter().position(|s| s == &sid) {
            self.active.remove(pos);
        }
        while self.active.len() >= MAX_MCP_SESSIONS {
            self.active.pop_front();
        }
        self.active.push_back(sid);
    }

    fn touch_and_validate(&mut self, sid: &str) -> bool {
        if let Some(pos) = self.active.iter().position(|s| s == sid) {
            if let Some(existing) = self.active.remove(pos) {
                self.active.push_back(existing);
            }
            true
        } else {
            false
        }
    }

    fn remove(&mut self, sid: &str) -> bool {
        if let Some(pos) = self.active.iter().position(|s| s == sid) {
            self.active.remove(pos);
            true
        } else {
            false
        }
    }
}

/// RAII concurrency permit decremented when a worker thread exits.
struct ConnPermit(Arc<AtomicUsize>);

impl Drop for ConnPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Handle to a running background localhost Streamable HTTP MCP server.
pub struct LocalMcpServer {
    addr: SocketAddr,
    stop_flag: Arc<AtomicBool>,
    service: Arc<McpService>,
    thread: Option<JoinHandle<()>>,
}

impl LocalMcpServer {
    /// Starts the local Streamable HTTP MCP server on `127.0.0.1:<port>`.
    pub fn start(port: u16, service: Arc<McpService>) -> Result<Self, String> {
        let bind_addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = TcpListener::bind(bind_addr)
            .map_err(|e| format!("Failed to bind 127.0.0.1:{port}: {e}"))?;
        let actual_addr = listener
            .local_addr()
            .map_err(|e| format!("Failed to read bound addr: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to configure socket: {e}"))?;

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = stop_flag.clone();
        let svc_clone = service.clone();
        let sessions = Arc::new(Mutex::new(SessionStore::new()));
        let active_conns = Arc::new(AtomicUsize::new(0));

        let thread = thread::Builder::new()
            .name(format!("rune-mcp-http-{}", actual_addr.port()))
            .spawn(move || {
                while !stop_clone.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _peer)) => {
                            if stop_clone.load(Ordering::Relaxed) {
                                break;
                            }
                            let acquired = active_conns
                                .try_update(Ordering::AcqRel, Ordering::Acquire, |cur| {
                                    (cur < MAX_CONCURRENT_HTTP_CONNS).then_some(cur + 1)
                                })
                                .is_ok();
                            if !acquired {
                                let _ = stream.set_nonblocking(false);
                                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                                let _ = write_response(
                                    &mut stream,
                                    "503 Service Unavailable",
                                    &[("Retry-After".to_owned(), "1".to_owned())],
                                    b"{\"error\":\"too many concurrent MCP connections\"}",
                                    "application/json",
                                    false,
                                );
                                let _ = stream.shutdown(Shutdown::Both);
                                continue;
                            }
                            let permit = ConnPermit(active_conns.clone());
                            let svc = svc_clone.clone();
                            let sess = sessions.clone();
                            let stop_conn = stop_clone.clone();
                            let _ = thread::Builder::new()
                                .name("rune-mcp-conn".to_owned())
                                .spawn(move || {
                                    let _permit = permit;
                                    let _ = handle_connection(stream, &svc, &sess, &stop_conn);
                                });
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(ref e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::Interrupted
                                    | std::io::ErrorKind::ConnectionAborted
                                    | std::io::ErrorKind::ConnectionReset
                            ) =>
                        {
                            continue;
                        }
                        Err(_) => {
                            // Transient accept error (e.g. EMFILE/ENFILE fd pressure): back off briefly
                            // rather than permanently terminating the local MCP listener.
                            thread::sleep(Duration::from_millis(25));
                        }
                    }
                }
            })
            .map_err(|e| format!("Failed to spawn MCP server thread: {e}"))?;

        Ok(Self {
            addr: actual_addr,
            stop_flag,
            service,
            thread: Some(thread),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn endpoint_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.addr.port())
    }

    pub fn request_count(&self) -> u64 {
        self.service.request_count()
    }

    pub fn last_tool(&self) -> Option<String> {
        self.service.last_tool()
    }

    /// Gracefully stops the listener thread and frees the local port immediately.
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(50));
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LocalMcpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

enum LineReadError {
    TooLong,
    InvalidUtf8,
    Io(std::io::Error),
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<String>, LineReadError> {
    let mut buf = Vec::new();
    let max_read = (max_bytes as u64).saturating_add(1);
    let n = reader
        .take(max_read)
        .read_until(b'\n', &mut buf)
        .map_err(LineReadError::Io)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.len() > max_bytes {
        return Err(LineReadError::TooLong);
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| LineReadError::InvalidUtf8)
}

fn handle_connection(
    mut stream: TcpStream,
    service: &McpService,
    sessions: &Mutex<SessionStore>,
    stop_flag: &AtomicBool,
) -> std::io::Result<()> {
    // On macOS/BSD, accepted streams inherit O_NONBLOCK from the non-blocking TcpListener.
    // Explicitly switch back to blocking mode so read/write timeouts and Keep-Alive work reliably!
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(ACTIVE_IO_TIMEOUT));

    let mut reader = BufReader::new(stream.try_clone()?);

    // HTTP/1.1 Keep-Alive loop: reuses pooled local connections cleanly across sequential
    // tool calls/exports, preventing transient ECONNRESET on Node undici/fetch/httpx clients.
    while !stop_flag.load(Ordering::Relaxed) {
        let _ = reader.get_ref().set_read_timeout(Some(IDLE_READ_TIMEOUT));
        let request_line = match read_bounded_line(&mut reader, MAX_HTTP_LINE_BYTES) {
            Ok(None) => break,
            Ok(Some(line)) => line,
            Err(LineReadError::TooLong) => {
                write_response(
                    &mut stream,
                    "414 URI Too Long",
                    &[],
                    b"{\"error\":\"request line exceeds 8 KiB limit\"}",
                    "application/json",
                    false,
                )?;
                break;
            }
            Err(LineReadError::InvalidUtf8) => {
                write_response(
                    &mut stream,
                    "400 Bad Request",
                    &[],
                    b"{\"error\":\"invalid UTF-8 in request line\"}",
                    "application/json",
                    false,
                )?;
                break;
            }
            Err(LineReadError::Io(ref e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break;
            }
            Err(LineReadError::Io(e)) => return Err(e),
        };

        let _ = reader.get_ref().set_read_timeout(Some(ACTIVE_IO_TIMEOUT));

        let trimmed_req = request_line.trim_end_matches(['\r', '\n']);
        let parts: Vec<&str> = trimmed_req.split_whitespace().collect();
        if parts.len() < 2 || parts.len() > 3 {
            write_response(
                &mut stream,
                "400 Bad Request",
                &[],
                b"{\"error\":\"malformed request line\"}",
                "application/json",
                false,
            )?;
            break;
        }
        let method = parts[0];
        let path = parts[1].split('?').next().unwrap_or(parts[1]);
        let http_version = parts.get(2).copied().unwrap_or("HTTP/1.1");

        let mut headers = HashMap::<String, String>::new();
        let mut header_count = 0usize;
        let mut host_count = 0usize;
        let mut content_length_count = 0usize;
        let mut has_transfer_encoding = false;
        let mut headers_too_large = false;
        let mut bad_header = false;
        let mut eof_mid_headers = false;

        loop {
            match read_bounded_line(&mut reader, MAX_HTTP_LINE_BYTES) {
                Ok(None) => {
                    eof_mid_headers = true;
                    break;
                }
                Ok(Some(line)) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    if trimmed.is_empty() {
                        break;
                    }
                    header_count += 1;
                    if header_count > MAX_HTTP_HEADERS {
                        headers_too_large = true;
                        break;
                    }
                    let Some((raw_k, raw_v)) = trimmed.split_once(':') else {
                        bad_header = true;
                        break;
                    };
                    if raw_k.is_empty()
                        || raw_k.bytes().any(|b| b <= 0x20 || b == 0x7F || b == b':')
                    {
                        bad_header = true;
                        break;
                    }
                    let key = raw_k.to_ascii_lowercase();
                    let val = raw_v.trim().to_owned();
                    match key.as_str() {
                        "host" => {
                            host_count += 1;
                            headers.insert(key, val);
                        }
                        "content-length" => {
                            content_length_count += 1;
                            headers.insert(key, val);
                        }
                        "transfer-encoding" => {
                            has_transfer_encoding = true;
                            headers
                                .entry(key)
                                .and_modify(|existing| {
                                    existing.push_str(", ");
                                    existing.push_str(&val);
                                })
                                .or_insert(val);
                        }
                        "connection" => {
                            headers
                                .entry(key)
                                .and_modify(|existing| {
                                    existing.push_str(", ");
                                    existing.push_str(&val);
                                })
                                .or_insert(val);
                        }
                        _ => {
                            headers.insert(key, val);
                        }
                    }
                }
                Err(LineReadError::TooLong) => {
                    headers_too_large = true;
                    break;
                }
                Err(LineReadError::InvalidUtf8) => {
                    bad_header = true;
                    break;
                }
                Err(LineReadError::Io(e)) => return Err(e),
            }
        }

        if eof_mid_headers {
            break;
        }
        if headers_too_large {
            write_response(
                &mut stream,
                "431 Request Header Fields Too Large",
                &[],
                b"{\"error\":\"request headers exceed limit\"}",
                "application/json",
                false,
            )?;
            break;
        }
        if bad_header {
            write_response(
                &mut stream,
                "400 Bad Request",
                &[],
                b"{\"error\":\"malformed HTTP header\"}",
                "application/json",
                false,
            )?;
            break;
        }

        // RFC 9112 §3.2: HTTP/1.1 requests MUST include a single valid `Host` header.
        if http_version.eq_ignore_ascii_case("HTTP/1.1") && host_count == 0 {
            write_response(
                &mut stream,
                "400 Bad Request",
                &[],
                b"{\"error\":\"missing required Host header\"}",
                "application/json",
                false,
            )?;
            break;
        }
        if host_count > 1 {
            write_response(
                &mut stream,
                "400 Bad Request",
                &[],
                b"{\"error\":\"duplicate Host header\"}",
                "application/json",
                false,
            )?;
            break;
        }

        // RFC 9112 §6.3: reject duplicate Content-Length or simultaneous Transfer-Encoding + Content-Length.
        if content_length_count > 1 {
            write_response(
                &mut stream,
                "400 Bad Request",
                &[],
                b"{\"error\":\"duplicate Content-Length header\"}",
                "application/json",
                false,
            )?;
            break;
        }
        if has_transfer_encoding && content_length_count > 0 {
            write_response(
                &mut stream,
                "400 Bad Request",
                &[],
                b"{\"error\":\"conflicting Transfer-Encoding and Content-Length headers\"}",
                "application/json",
                false,
            )?;
            break;
        }
        let parsed_content_length: Option<usize> = if content_length_count == 1 {
            let raw_cl = headers
                .get("content-length")
                .map(String::as_str)
                .unwrap_or("");
            if raw_cl.is_empty() || !raw_cl.bytes().all(|b| b.is_ascii_digit()) {
                write_response(
                    &mut stream,
                    "400 Bad Request",
                    &[],
                    b"{\"error\":\"invalid Content-Length header\"}",
                    "application/json",
                    false,
                )?;
                break;
            }
            match raw_cl.parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => {
                    write_response(
                        &mut stream,
                        "400 Bad Request",
                        &[],
                        b"{\"error\":\"invalid Content-Length header\"}",
                        "application/json",
                        false,
                    )?;
                    break;
                }
            }
        } else {
            None
        };

        let client_conn = headers
            .get("connection")
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let has_unread_body_for_non_post =
            method != "POST" && (parsed_content_length.unwrap_or(0) > 0 || has_transfer_encoding);
        let keep_alive = if has_unread_body_for_non_post {
            false
        } else if http_version.eq_ignore_ascii_case("HTTP/1.0") {
            client_conn.contains("keep-alive")
        } else {
            !client_conn.contains("close")
        };

        // Defense-in-depth DNS-rebinding protection: validate both `Host` and `Origin` headers.
        if let Some(host) = headers.get("host").map(String::as_str)
            && !is_allowed_local_host(host)
        {
            let err = jsonrpc_error(
                None,
                rpc_err::INVALID_REQUEST,
                format!("Forbidden Host '{host}': Rune Darkroom MCP accepts localhost only."),
            );
            let body = serde_json::to_vec(&err).unwrap_or_default();
            write_response(
                &mut stream,
                "403 Forbidden",
                &[],
                &body,
                "application/json",
                false,
            )?;
            break;
        }

        // Spec requirement (MCP 2025-11-25): MUST validate Origin on all HTTP requests (403 if untrusted).
        let origin = headers.get("origin").map(String::as_str);
        if let Some(orig) = origin
            && !is_allowed_local_origin(orig)
        {
            let err = jsonrpc_error(
                None,
                rpc_err::INVALID_REQUEST,
                format!(
                    "Forbidden Origin '{orig}': Rune Darkroom MCP accepts localhost origins only."
                ),
            );
            let body = serde_json::to_vec(&err).unwrap_or_default();
            write_response(
                &mut stream,
                "403 Forbidden",
                &[],
                &body,
                "application/json",
                false,
            )?;
            break;
        }

        let mut cors_headers: Vec<(String, String)> = vec![
            (
                "MCP-Protocol-Version".to_owned(),
                MCP_PROTOCOL_LATEST.to_owned(),
            ),
            ("Vary".to_owned(), "Origin".to_owned()),
        ];
        if let Some(orig) = origin {
            cors_headers.push(("Access-Control-Allow-Origin".to_owned(), orig.to_owned()));
            cors_headers.push((
                "Access-Control-Expose-Headers".to_owned(),
                "Mcp-Session-Id, MCP-Protocol-Version".to_owned(),
            ));
        }

        // Health check route (convenience for local tooling)
        if path == "/health" && method == "GET" {
            let body = serde_json::to_vec(&json!({
                "ok": true,
                "server": "rune-darkroom",
                "protocolVersion": MCP_PROTOCOL_LATEST,
                "requests": service.request_count(),
            }))
            .unwrap_or_default();
            write_response(
                &mut stream,
                "200 OK",
                &cors_headers,
                &body,
                "application/json",
                keep_alive,
            )?;
            if !keep_alive {
                break;
            }
            continue;
        }

        if path != "/mcp" && path != "/" {
            write_response(
                &mut stream,
                "404 Not Found",
                &cors_headers,
                b"{\"error\":\"use /mcp\"}",
                "application/json",
                keep_alive,
            )?;
            if !keep_alive {
                break;
            }
            continue;
        }

        match method {
            "OPTIONS" => {
                cors_headers.push(("Allow".to_owned(), "POST, DELETE, OPTIONS".to_owned()));
                cors_headers.push((
                    "Access-Control-Allow-Methods".to_owned(),
                    "POST, DELETE, OPTIONS".to_owned(),
                ));
                cors_headers.push((
                    "Access-Control-Allow-Headers".to_owned(),
                    "Content-Type, Accept, Mcp-Session-Id, MCP-Protocol-Version, Authorization"
                        .to_owned(),
                ));
                write_response(
                    &mut stream,
                    "204 No Content",
                    &cors_headers,
                    b"",
                    "text/plain",
                    keep_alive,
                )?;
            }
            "GET" => {
                // MCP 2025-11-25 Streamable HTTP: return 405 when server does not open an unsolicited GET SSE stream.
                cors_headers.push(("Allow".to_owned(), "POST, DELETE, OPTIONS".to_owned()));
                write_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    &cors_headers,
                    b"{\"error\":\"GET SSE stream not offered; use POST /mcp\"}",
                    "application/json",
                    keep_alive,
                )?;
            }
            "DELETE" => {
                let status = if let Some(sid) = headers.get("mcp-session-id") {
                    let ok = sessions
                        .lock()
                        .map(|mut store| store.remove(sid) || !store.sessions_issued)
                        .unwrap_or(false);
                    if ok {
                        "204 No Content"
                    } else {
                        "404 Not Found"
                    }
                } else {
                    "204 No Content"
                };
                write_response(
                    &mut stream,
                    status,
                    &cors_headers,
                    b"",
                    "text/plain",
                    keep_alive,
                )?;
            }
            "POST" => {
                // MCP 2025-11-25: reject unsupported MCP-Protocol-Version header with 400 Bad Request.
                if let Some(proto_ver) = headers.get("mcp-protocol-version")
                    && !is_supported_protocol_version(proto_ver)
                {
                    let err = jsonrpc_error(
                        None,
                        rpc_err::INVALID_REQUEST,
                        format!(
                            "Unsupported MCP-Protocol-Version '{proto_ver}'. Supported: {}",
                            SUPPORTED_PROTOCOL_VERSIONS.join(", ")
                        ),
                    );
                    let payload = serde_json::to_vec(&err).unwrap_or_default();
                    write_response(
                        &mut stream,
                        "400 Bad Request",
                        &cors_headers,
                        &payload,
                        "application/json",
                        false,
                    )?;
                    break;
                }

                let is_chunked = if has_transfer_encoding {
                    let final_coding = headers
                        .get("transfer-encoding")
                        .and_then(|v| v.rsplit(',').next())
                        .map(str::trim)
                        .unwrap_or("");
                    if !final_coding.eq_ignore_ascii_case("chunked") {
                        write_response(
                            &mut stream,
                            "400 Bad Request",
                            &cors_headers,
                            b"{\"error\":\"unsupported Transfer-Encoding\"}",
                            "application/json",
                            false,
                        )?;
                        break;
                    }
                    true
                } else {
                    false
                };

                if let Some(expect) = headers.get("expect")
                    && expect.eq_ignore_ascii_case("100-continue")
                {
                    stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
                    stream.flush()?;
                }

                let body_bytes = if is_chunked {
                    match read_chunked_body(&mut reader, MAX_HTTP_BODY_BYTES) {
                        Ok(b) => b,
                        Err(ChunkReadError::TooLarge) => {
                            write_response(
                                &mut stream,
                                "413 Payload Too Large",
                                &cors_headers,
                                b"{\"error\":\"chunked body exceeds 4 MiB limit\"}",
                                "application/json",
                                false,
                            )?;
                            break;
                        }
                        Err(ChunkReadError::BadFraming) => {
                            write_response(
                                &mut stream,
                                "400 Bad Request",
                                &cors_headers,
                                b"{\"error\":\"malformed chunked transfer encoding\"}",
                                "application/json",
                                false,
                            )?;
                            break;
                        }
                        Err(ChunkReadError::Io(e)) => return Err(e),
                    }
                } else {
                    let content_len = parsed_content_length.unwrap_or(0);
                    if content_len > MAX_HTTP_BODY_BYTES {
                        write_response(
                            &mut stream,
                            "413 Payload Too Large",
                            &cors_headers,
                            b"{\"error\":\"body exceeds 4 MiB limit\"}",
                            "application/json",
                            false,
                        )?;
                        break;
                    }
                    let mut buf = vec![0u8; content_len];
                    reader.read_exact(&mut buf)?;
                    buf
                };

                let req: JsonRpcRequest = match serde_json::from_slice(&body_bytes) {
                    Ok(r) => r,
                    Err(e) => {
                        let err =
                            jsonrpc_error(None, rpc_err::PARSE_ERROR, format!("Parse error: {e}"));
                        let payload = serde_json::to_vec(&err).unwrap_or_default();
                        write_response(
                            &mut stream,
                            "400 Bad Request",
                            &cors_headers,
                            &payload,
                            "application/json",
                            keep_alive,
                        )?;
                        if !keep_alive {
                            break;
                        }
                        continue;
                    }
                };

                if !req.is_valid_jsonrpc() {
                    let err = jsonrpc_error(
                        req.id.clone(),
                        rpc_err::INVALID_REQUEST,
                        format!(
                            "Invalid JSON-RPC version '{}': expected \"2.0\".",
                            req.jsonrpc
                        ),
                    );
                    let payload = serde_json::to_vec(&err).unwrap_or_default();
                    write_response(
                        &mut stream,
                        "400 Bad Request",
                        &cors_headers,
                        &payload,
                        "application/json",
                        keep_alive,
                    )?;
                    if !keep_alive {
                        break;
                    }
                    continue;
                }

                let is_init = req.method == "initialize";
                if is_init {
                    let sid = generate_session_id();
                    if let Ok(mut store) = sessions.lock() {
                        store.issue(sid.clone());
                    }
                    cors_headers.push(("Mcp-Session-Id".to_owned(), sid));
                } else if let Some(sid) = headers.get("mcp-session-id") {
                    let valid = sessions
                        .lock()
                        .map(|mut store| store.touch_and_validate(sid))
                        .unwrap_or(false);
                    if !valid {
                        let err = jsonrpc_error(
                            req.id.clone(),
                            rpc_err::INVALID_REQUEST,
                            "Unknown or terminated Mcp-Session-Id; re-run `initialize`.",
                        );
                        let payload = serde_json::to_vec(&err).unwrap_or_default();
                        write_response(
                            &mut stream,
                            "404 Not Found",
                            &cors_headers,
                            &payload,
                            "application/json",
                            keep_alive,
                        )?;
                        if !keep_alive {
                            break;
                        }
                        continue;
                    }
                    cors_headers.push(("Mcp-Session-Id".to_owned(), sid.clone()));
                }

                match service.handle_request(&req) {
                    None => {
                        write_response(
                            &mut stream,
                            "202 Accepted",
                            &cors_headers,
                            b"",
                            "text/plain",
                            keep_alive,
                        )?;
                    }
                    Some(resp) => {
                        let negotiated = resp
                            .get("result")
                            .and_then(|r| r.get("protocolVersion"))
                            .and_then(Value::as_str);
                        if let Some(ver) = negotiated
                            && let Some((_, v)) = cors_headers
                                .iter_mut()
                                .find(|(k, _)| k.eq_ignore_ascii_case("MCP-Protocol-Version"))
                        {
                            *v = ver.to_owned();
                        }
                        let payload = serde_json::to_vec(&resp).unwrap_or_default();
                        write_response(
                            &mut stream,
                            "200 OK",
                            &cors_headers,
                            &payload,
                            "application/json",
                            keep_alive,
                        )?;
                    }
                }
            }
            _ => {
                cors_headers.push(("Allow".to_owned(), "POST, DELETE, OPTIONS".to_owned()));
                write_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    &cors_headers,
                    b"{\"error\":\"method not allowed\"}",
                    "application/json",
                    keep_alive,
                )?;
            }
        }

        if !keep_alive {
            break;
        }
    }

    let _ = stream.shutdown(Shutdown::Write);
    Ok(())
}

fn is_valid_port(port: &str) -> bool {
    !port.is_empty()
        && !port.starts_with('+')
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok()
}

/// Validates that an HTTP `Host` header refers strictly to localhost with an optional valid `u16` port.
pub fn is_allowed_local_host(host: &str) -> bool {
    let raw = host.trim();
    if raw.is_empty()
        || raw
            .bytes()
            .any(|b| b <= 0x20 || b == 0x7F || matches!(b, b'@' | b'/' | b'?' | b'#'))
    {
        return false;
    }

    if let Some(after_v6) = raw.strip_prefix("[::1]") {
        return after_v6.is_empty() || after_v6.strip_prefix(':').is_some_and(is_valid_port);
    }

    let (hostname, opt_port) = match raw.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (raw, None),
    };

    let is_loopback_name = hostname.eq_ignore_ascii_case("localhost") || hostname == "127.0.0.1";
    if !is_loopback_name {
        return false;
    }
    opt_port.is_none_or(is_valid_port)
}

/// Validates that an `Origin` header is strictly an RFC 6454 localhost origin (`http(s)://host[:port]`).
pub fn is_allowed_local_origin(origin: &str) -> bool {
    let trimmed = origin.trim();
    if trimmed.is_empty()
        || trimmed
            .bytes()
            .any(|b| b <= 0x20 || b == 0x7F || matches!(b, b'@' | b'?' | b'#'))
    {
        return false;
    }
    let rest = if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("http://") {
        &trimmed[7..]
    } else if trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("https://") {
        &trimmed[8..]
    } else {
        return false;
    };
    if rest.contains('/') {
        return false;
    }
    is_allowed_local_host(rest)
}

enum ChunkReadError {
    TooLarge,
    BadFraming,
    Io(std::io::Error),
}

fn read_chunked_body<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Vec<u8>, ChunkReadError> {
    let mut out = Vec::new();
    loop {
        let size_line = match read_bounded_line(reader, MAX_HTTP_LINE_BYTES) {
            Ok(Some(line)) => line,
            Ok(None) | Err(LineReadError::TooLong | LineReadError::InvalidUtf8) => {
                return Err(ChunkReadError::BadFraming);
            }
            Err(LineReadError::Io(e)) => return Err(ChunkReadError::Io(e)),
        };
        if !size_line.ends_with('\n') {
            return Err(ChunkReadError::BadFraming);
        }
        let hex = size_line.trim().split(';').next().unwrap_or("").trim();
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ChunkReadError::BadFraming);
        }
        let chunk_len = usize::from_str_radix(hex, 16).map_err(|_| ChunkReadError::BadFraming)?;
        if chunk_len == 0 {
            // Drain optional trailer headers (capped at MAX_HTTP_TRAILERS) and final CRLF.
            let mut trailer_count = 0usize;
            loop {
                let trailer = match read_bounded_line(reader, MAX_HTTP_LINE_BYTES) {
                    Ok(Some(line)) => line,
                    Ok(None) | Err(LineReadError::TooLong | LineReadError::InvalidUtf8) => {
                        return Err(ChunkReadError::BadFraming);
                    }
                    Err(LineReadError::Io(e)) => return Err(ChunkReadError::Io(e)),
                };
                if trailer == "\r\n" || trailer == "\n" {
                    break;
                }
                trailer_count += 1;
                if trailer_count > MAX_HTTP_TRAILERS || !trailer.ends_with('\n') {
                    return Err(ChunkReadError::BadFraming);
                }
            }
            return Ok(out);
        }
        if out.len().saturating_add(chunk_len) > max_bytes {
            return Err(ChunkReadError::TooLarge);
        }
        let start = out.len();
        out.resize(start + chunk_len, 0);
        reader
            .read_exact(&mut out[start..])
            .map_err(ChunkReadError::Io)?;
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).map_err(ChunkReadError::Io)?;
        if crlf != *b"\r\n" {
            return Err(ChunkReadError::BadFraming);
        }
    }
}

fn read_os_random_bytes(buf: &mut [u8]) -> bool {
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .is_ok()
}

/// Generates a visible-ASCII (`0x21..=0x7E`) unique `Mcp-Session-Id` from OS CSPRNG
/// (`/dev/urandom`, 16 random bytes hex) with SplitMix64 fallback if unavailable.
fn generate_session_id() -> String {
    let mut bytes = [0u8; 16];
    if read_os_random_bytes(&mut bytes) {
        let mut sid = String::with_capacity(3 + 32);
        sid.push_str("rd-");
        for b in bytes {
            let _ = write!(&mut sid, "{b:02x}");
        }
        return sid;
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut x = (nanos as u64) ^ ((pid as u64) << 32) ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let a = x ^ (x >> 31);
    let b = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (nanos >> 64) as u64;
    format!("rd-{a:016x}-{b:016x}")
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(String, String)],
    body: &[u8],
    content_type: &str,
    keep_alive: bool,
) -> std::io::Result<()> {
    let conn = if keep_alive { "keep-alive" } else { "close" };
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: {conn}\r\n",
        body.len()
    );
    if keep_alive {
        head.push_str("Keep-Alive: timeout=10\r\n");
    }
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if !body.is_empty() {
        stream.write_all(body)?;
    }
    stream.flush()
}
