//! Download-on-command, offline local setup, license-gate enforcement, and safe removal
//! for optional ONNX segmentation models (SAM 2.1 Hiera Tiny & SAM 3 Tracker).
//!
//! Uses system `curl` with strict HTTPS-only protocol & redirect enforcement (`-fsSL`,
//! `--proto =https --proto-redir =https`), bounded asynchronous stderr draining (eliminating
//! pipe-buffer deadlocks on large downloads), RAII child-process cleanup (no zombies on panic
//! or error), stale `.part` HTTP 416 fresh-retry, atomic `.part` -> `sync_all` -> `rename` writes,
//! symlink/path-traversal guards, and automatic ONNX Runtime session cache eviction.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rune_editor_core::{
    AiModelConfig, CoreError, ModelId, ModelStatus, SegmentationEngine, inspect_all_model_statuses,
    inspect_model_install, inspect_model_status, load_ai_model_config, model_dir_in,
    models_root_dir, save_ai_model_config,
};
use serde::Serialize;
use thiserror::Error;

/// Cap captured `curl` stderr bytes so an error stream can never exhaust RAM or block the pipe.
const MAX_CURL_STDERR_BYTES: usize = 8 * 1024;

/// Errors returned by model download, offline setup, and removal workflows.
#[derive(Debug, Error)]
pub enum ModelError {
    #[error(
        "Model '{model_id}' requires accepting the {license} ({license_url}) before downloading or installing. Re-run with license acceptance enabled (e.g. `--accept-license`)."
    )]
    LicenseRequired {
        model_id: ModelId,
        license: String,
        license_url: String,
    },
    #[error("Download of model '{0}' was cancelled")]
    Cancelled(ModelId),
    #[error("Invalid model file path: {0}")]
    InvalidPath(String),
    #[error("Required file '{filename}' for model '{model_id}' not found in '{path}'")]
    MissingLocalFile {
        model_id: ModelId,
        filename: String,
        path: PathBuf,
    },
    #[error("Model file '{filename}' is empty or corrupt")]
    EmptyOrCorruptFile { filename: String },
    #[error("Failed to download '{filename}' for model '{model_id}': {message}")]
    DownloadFailed {
        model_id: ModelId,
        filename: String,
        message: String,
    },
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Core config error: {0}")]
    Core(#[from] CoreError),
}

/// Live download progress update reported to GUI progress bars, CLI output, or callers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelDownloadProgress {
    pub model_id: ModelId,
    pub file_index: usize,
    pub total_files: usize,
    pub file_name: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub overall_fraction: f32,
    pub status: String,
}

/// Serializable summary of the active & effective segmentation engines for GUI, CLI, and MCP.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActiveEngineSummary {
    pub configured_engine: SegmentationEngine,
    pub effective_auto_engine: SegmentationEngine,
    pub effective_interactive_engine: SegmentationEngine,
    pub apple_vision_available: bool,
    pub apple_vision_label: String,
    pub installed_models: Vec<ModelId>,
    pub preferred_sam3_variant: ModelId,
    pub models_root: PathBuf,
}

/// RAII guard guaranteeing a spawned `curl` child is killed and reaped on every exit path
/// (including early `?` returns, cancellation, or panics inside `on_progress`).
struct ChildReapGuard {
    child: Option<Child>,
}

impl ChildReapGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.as_mut().and_then(|c| c.stderr.take())
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        if let Some(status) = child.try_wait()? {
            self.child = None;
            Ok(Some(status))
        } else {
            Ok(None)
        }
    }

    fn kill_and_wait(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ChildReapGuard {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

/// Spawns a tiny background thread that drains `stderr` to EOF (preventing pipe-buffer deadlocks)
/// while retaining at most `MAX_CURL_STDERR_BYTES` for error diagnostics.
fn spawn_stderr_drainer(stderr: Option<std::process::ChildStderr>) -> Option<JoinHandle<String>> {
    let mut pipe = stderr?;
    Some(std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if kept.len() < MAX_CURL_STDERR_BYTES {
                        let room = MAX_CURL_STDERR_BYTES - kept.len();
                        kept.extend_from_slice(&buf[..n.min(room)]);
                    }
                }
            }
        }
        String::from_utf8_lossy(&kept).into_owned()
    }))
}

fn finish_stderr_drainer(handle: Option<JoinHandle<String>>) -> String {
    handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Downloads all ONNX weights & graphs for `id` into `model_dir(id)` atomically via `curl`.
///
/// Rejects models with `requires_license_acceptance == true` unless `accept_license == true`.
/// Enforces HTTPS-only protocols & redirects, rejects symlinks at destination paths, supports
/// resumable `.part` downloads with HTTP 416 fresh-retry, live progress callbacks, and
/// zombie-free cancellation.
pub fn download_model(
    id: ModelId,
    accept_license: bool,
    cancel: Option<Arc<AtomicBool>>,
    on_progress: impl Fn(ModelDownloadProgress),
) -> Result<ModelStatus, ModelError> {
    let spec = id.spec();
    enforce_license_gate(id, accept_license)?;

    let root = models_root_dir();
    let target_dir = safe_model_dir(&root, id)?;
    reject_if_symlink(&target_dir)?;
    fs::create_dir_all(&target_dir)?;
    reject_if_symlink(&target_dir)?;

    let total_files = spec.files.len();
    let total_expected_bytes = spec.approx_bytes.max(1);
    let mut completed_bytes = 0u64;

    for (file_index, file_spec) in spec.files.iter().enumerate() {
        if is_cancelled(cancel.as_ref()) {
            return Err(ModelError::Cancelled(id));
        }

        if !file_spec.url.starts_with("https://") {
            return Err(ModelError::DownloadFailed {
                model_id: id,
                filename: file_spec.filename.to_owned(),
                message: "Refusing non-HTTPS model download URL".to_owned(),
            });
        }

        let dest_path = safe_child_path(&target_dir, file_spec.filename)?;
        let part_name = format!("{}.part", file_spec.filename);
        let part_path = safe_child_path(&target_dir, &part_name)?;
        reject_if_symlink(&dest_path)?;
        reject_if_symlink(&part_path)?;

        // Skip files that are already finalized, regular, and non-empty (resuming a partial install).
        if let Ok(meta) = fs::symlink_metadata(&dest_path)
            && meta.is_file()
            && meta.len() > 0
        {
            completed_bytes = completed_bytes.saturating_add(meta.len());
            on_progress(ModelDownloadProgress {
                model_id: id,
                file_index,
                total_files,
                file_name: file_spec.filename.to_owned(),
                bytes_downloaded: meta.len(),
                total_bytes: file_spec.approx_bytes.max(meta.len()),
                overall_fraction: (completed_bytes as f64 / total_expected_bytes as f64)
                    .clamp(0.0, 0.99) as f32,
                status: format!("Verified existing {}", file_spec.filename),
            });
            continue;
        }

        let had_existing_part = fs::symlink_metadata(&part_path).is_ok();
        let mut attempt_resume = had_existing_part;
        let mut attempt = 0u8;

        loop {
            attempt += 1;
            let (status, stderr_text) = run_curl_file_download(
                id,
                file_index,
                total_files,
                file_spec.filename,
                file_spec.url,
                file_spec.approx_bytes,
                completed_bytes,
                total_expected_bytes,
                &part_path,
                attempt_resume,
                cancel.as_ref(),
                &on_progress,
            )?;

            if status.success() {
                break;
            }

            // If resuming a stale `.part` file fails (e.g. HTTP 416 Range Not Satisfiable or
            // curl exit code 33), delete `.part` and retry once fresh without `--continue-at -`.
            let looks_like_range_err = attempt_resume
                && (status.code() == Some(33)
                    || stderr_text.contains("416")
                    || stderr_text.to_ascii_lowercase().contains("range"));
            if looks_like_range_err && attempt == 1 {
                let _ = fs::remove_file(&part_path);
                attempt_resume = false;
                continue;
            }

            let detail = if stderr_text.is_empty() {
                format!("curl exited with status {status}")
            } else {
                stderr_text.chars().take(400).collect()
            };
            return Err(ModelError::DownloadFailed {
                model_id: id,
                filename: file_spec.filename.to_owned(),
                message: detail,
            });
        }

        reject_if_symlink(&part_path)?;
        validate_file_contents(&part_path, file_spec.filename, true)?;
        let final_len = fs::metadata(&part_path)?.len();
        // Flush downloaded bytes to disk before atomic rename.
        File::open(&part_path)?.sync_all()?;
        reject_if_symlink(&dest_path)?;
        fs::rename(&part_path, &dest_path)?;
        completed_bytes = completed_bytes.saturating_add(final_len);
    }

    let mut config = load_ai_model_config().unwrap_or_default();
    if spec.requires_license_acceptance && accept_license {
        config.accept_license(id);
        save_ai_model_config(&config)?;
    }

    crate::sam_ort::evict_model_from_cache(id);

    on_progress(ModelDownloadProgress {
        model_id: id,
        file_index: total_files.saturating_sub(1),
        total_files,
        file_name: spec
            .files
            .last()
            .map(|f| f.filename.to_owned())
            .unwrap_or_default(),
        bytes_downloaded: completed_bytes,
        total_bytes: completed_bytes.max(total_expected_bytes),
        overall_fraction: 1.0,
        status: format!("{} installed", spec.display_name),
    });

    Ok(inspect_model_status(id, &config))
}

#[allow(clippy::too_many_arguments)]
fn run_curl_file_download(
    id: ModelId,
    file_index: usize,
    total_files: usize,
    filename: &str,
    url: &str,
    approx_file_bytes: u64,
    completed_bytes: u64,
    total_expected_bytes: u64,
    part_path: &Path,
    resume: bool,
    cancel: Option<&Arc<AtomicBool>>,
    on_progress: &impl Fn(ModelDownloadProgress),
) -> Result<(ExitStatus, String), ModelError> {
    on_progress(ModelDownloadProgress {
        model_id: id,
        file_index,
        total_files,
        file_name: filename.to_owned(),
        bytes_downloaded: fs::metadata(part_path).map(|m| m.len()).unwrap_or(0),
        total_bytes: approx_file_bytes,
        overall_fraction: (completed_bytes as f64 / total_expected_bytes as f64).clamp(0.0, 0.99)
            as f32,
        status: format!("Downloading {filename}"),
    });

    let mut cmd = Command::new("curl");
    // `-fsSL`: fail on HTTP errors, silent (no progress meter spamming stderr), still show errors,
    // follow redirects — strictly locked to HTTPS for both initial request and all redirects.
    cmd.arg("-fsSL")
        .arg("--proto")
        .arg("=https")
        .arg("--proto-redir")
        .arg("=https")
        .arg("--retry")
        .arg("2")
        .arg("--connect-timeout")
        .arg("15");
    if resume {
        cmd.arg("--continue-at").arg("-");
    }
    cmd.arg("-o")
        .arg(part_path)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(|e| ModelError::DownloadFailed {
        model_id: id,
        filename: filename.to_owned(),
        message: format!("Failed to spawn curl: {e}"),
    })?;

    let mut child_guard = ChildReapGuard::new(child);
    let stderr_handle = spawn_stderr_drainer(child_guard.take_stderr());

    let mut last_reported_bytes = u64::MAX;
    let mut last_report_time = Instant::now();

    let status = loop {
        if is_cancelled(cancel) {
            child_guard.kill_and_wait();
            let _ = finish_stderr_drainer(stderr_handle);
            return Err(ModelError::Cancelled(id));
        }

        if let Some(exit) = child_guard.try_wait()? {
            break exit;
        }

        let cur_bytes = fs::metadata(part_path).map(|m| m.len()).unwrap_or(0);
        if cur_bytes != last_reported_bytes
            || last_report_time.elapsed() >= Duration::from_millis(250)
        {
            last_reported_bytes = cur_bytes;
            last_report_time = Instant::now();
            let overall = (completed_bytes.saturating_add(cur_bytes) as f64
                / total_expected_bytes as f64)
                .clamp(0.0, 0.99) as f32;
            on_progress(ModelDownloadProgress {
                model_id: id,
                file_index,
                total_files,
                file_name: filename.to_owned(),
                bytes_downloaded: cur_bytes,
                total_bytes: approx_file_bytes.max(cur_bytes),
                overall_fraction: overall,
                status: format!("Downloading {filename}"),
            });
        }

        std::thread::sleep(Duration::from_millis(80));
    };

    let stderr_text = finish_stderr_drainer(stderr_handle);
    Ok((status, stderr_text))
}

/// Installs all required ONNX files for `id` from an offline local directory atomically.
///
/// Checks both `source_dir/<filename>` and `source_dir/onnx/<filename>` (matching Hugging Face
/// repository clones and HF Hub symlinked snapshots), pre-validates every expected source file
/// (non-empty and non-HTML) before writing anything, rejects symlinks at destination paths, and
/// enforces the license gate for models requiring explicit acceptance.
pub fn setup_model_from_local_dir(
    id: ModelId,
    source_dir: &Path,
    accept_license: bool,
) -> Result<ModelStatus, ModelError> {
    let spec = id.spec();
    enforce_license_gate(id, accept_license)?;

    if !source_dir.is_dir() {
        return Err(ModelError::InvalidPath(format!(
            "Local model source '{}' is not an existing directory",
            source_dir.display()
        )));
    }

    // Verify all required source files exist, are non-empty, and are not HTML error pages
    // BEFORE creating or modifying any files in the destination directory.
    let mut resolved_sources: Vec<(&str, PathBuf)> = Vec::with_capacity(spec.files.len());
    for file_spec in spec.files {
        let direct = source_dir.join(file_spec.filename);
        let nested = source_dir.join("onnx").join(file_spec.filename);
        let src_path = if direct.is_file() {
            direct
        } else if nested.is_file() {
            nested
        } else {
            return Err(ModelError::MissingLocalFile {
                model_id: id,
                filename: file_spec.filename.to_owned(),
                path: source_dir.to_path_buf(),
            });
        };

        validate_file_contents(&src_path, file_spec.filename, false)?;
        resolved_sources.push((file_spec.filename, src_path));
    }

    let root = models_root_dir();
    let target_dir = safe_model_dir(&root, id)?;
    reject_if_symlink(&target_dir)?;
    fs::create_dir_all(&target_dir)?;
    reject_if_symlink(&target_dir)?;

    for (filename, src_path) in resolved_sources {
        let dest_path = safe_child_path(&target_dir, filename)?;
        let part_name = format!("{filename}.part");
        let part_path = safe_child_path(&target_dir, &part_name)?;
        reject_if_symlink(&dest_path)?;
        reject_if_symlink(&part_path)?;

        let copy_result = (|| -> io::Result<()> {
            let mut reader = File::open(&src_path)?;
            let mut writer = File::create(&part_path)?;
            io::copy(&mut reader, &mut writer)?;
            writer.sync_all()?;
            Ok(())
        })();

        if let Err(err) = copy_result {
            let _ = fs::remove_file(&part_path);
            return Err(ModelError::Io(err));
        }

        validate_file_contents(&part_path, filename, true)?;
        reject_if_symlink(&dest_path)?;
        fs::rename(&part_path, &dest_path)?;
    }

    let mut config = load_ai_model_config().unwrap_or_default();
    if spec.requires_license_acceptance && accept_license {
        config.accept_license(id);
        save_ai_model_config(&config)?;
    }

    crate::sam_ort::evict_model_from_cache(id);

    Ok(inspect_model_status(id, &config))
}

/// Safely removes the installed model directory for `id` (strictly inside `models_root_dir()`)
/// and evicts any cached ONNX Runtime sessions for it.
pub fn remove_model(id: ModelId) -> Result<(), ModelError> {
    let root = models_root_dir();
    let target_dir = safe_model_dir(&root, id)?;

    match fs::symlink_metadata(&target_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(ModelError::InvalidPath(format!(
                    "Refusing to remove symlinked model directory '{}'",
                    target_dir.display()
                )));
            }
            if meta.is_dir() {
                fs::remove_dir_all(&target_dir)?;
            } else {
                return Err(ModelError::InvalidPath(format!(
                    "Expected model directory at '{}', found non-directory",
                    target_dir.display()
                )));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(ModelError::Io(err)),
    }

    crate::sam_ort::evict_session_cache();
    Ok(())
}

/// Lists installation and license statuses for every cataloged downloadable model.
pub fn list_model_statuses() -> Vec<ModelStatus> {
    let config = load_ai_model_config().unwrap_or_default();
    inspect_all_model_statuses(&config)
}

/// Returns a summary of the configured engine, resolved effective engines for Auto & Interactive
/// modes, Apple Vision availability, and installed ONNX models.
pub fn active_engine_summary() -> ActiveEngineSummary {
    let config = load_ai_model_config().unwrap_or_default();
    active_engine_summary_with_config(&config)
}

pub(crate) fn active_engine_summary_with_config(config: &AiModelConfig) -> ActiveEngineSummary {
    let installed_models: Vec<ModelId> = ModelId::ALL
        .iter()
        .copied()
        .filter(|&id| inspect_model_install(id).is_installed())
        .collect();

    #[cfg(target_os = "macos")]
    let (apple_vision_available, apple_vision_interactive, apple_vision_label) = (
        crate::vision_apple::is_available(),
        crate::vision_apple::supports_interactive(),
        crate::vision_apple::available_backend_label().to_owned(),
    );

    #[cfg(not(target_os = "macos"))]
    let (apple_vision_available, apple_vision_interactive, apple_vision_label) =
        (false, false, "Apple Vision (macOS only)".to_owned());

    let has_sam3 = installed_models.iter().any(|id| id.is_sam3());
    let has_sam21 = installed_models.contains(&ModelId::Sam21HieraTinyFp16);

    let resolve_auto_subject = || -> SegmentationEngine {
        if apple_vision_available {
            SegmentationEngine::AppleVision
        } else if has_sam3 {
            SegmentationEngine::Sam3Tracker
        } else if has_sam21 {
            SegmentationEngine::Sam21Tiny
        } else {
            SegmentationEngine::Heuristic
        }
    };

    let resolve_interactive = || -> SegmentationEngine {
        if has_sam3 {
            SegmentationEngine::Sam3Tracker
        } else if has_sam21 {
            SegmentationEngine::Sam21Tiny
        } else if apple_vision_interactive {
            SegmentationEngine::AppleVision
        } else {
            SegmentationEngine::Heuristic
        }
    };

    let effective_auto_engine = match config.active_engine {
        SegmentationEngine::Auto => resolve_auto_subject(),
        SegmentationEngine::AppleVision if apple_vision_available => {
            SegmentationEngine::AppleVision
        }
        SegmentationEngine::Sam3Tracker if has_sam3 => SegmentationEngine::Sam3Tracker,
        SegmentationEngine::Sam21Tiny if has_sam21 => SegmentationEngine::Sam21Tiny,
        SegmentationEngine::Heuristic => SegmentationEngine::Heuristic,
        _ => resolve_auto_subject(),
    };

    let effective_interactive_engine = match config.active_engine {
        SegmentationEngine::Auto => resolve_interactive(),
        SegmentationEngine::AppleVision if apple_vision_interactive => {
            SegmentationEngine::AppleVision
        }
        SegmentationEngine::Sam3Tracker if has_sam3 => SegmentationEngine::Sam3Tracker,
        SegmentationEngine::Sam21Tiny if has_sam21 => SegmentationEngine::Sam21Tiny,
        SegmentationEngine::Heuristic => SegmentationEngine::Heuristic,
        _ => resolve_interactive(),
    };

    ActiveEngineSummary {
        configured_engine: config.active_engine,
        effective_auto_engine,
        effective_interactive_engine,
        apple_vision_available,
        apple_vision_label,
        installed_models,
        preferred_sam3_variant: config.preferred_sam3_variant,
        models_root: models_root_dir(),
    }
}

fn enforce_license_gate(id: ModelId, accept_license: bool) -> Result<(), ModelError> {
    let spec = id.spec();
    if spec.requires_license_acceptance && !accept_license {
        return Err(ModelError::LicenseRequired {
            model_id: id,
            license: spec.license.to_owned(),
            license_url: spec.license_url.to_owned(),
        });
    }
    Ok(())
}

fn is_cancelled(cancel: Option<&Arc<AtomicBool>>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

fn reject_if_symlink(path: &Path) -> Result<(), ModelError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(ModelError::InvalidPath(format!(
            "Refusing to operate on symlinked model path '{}'",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(ModelError::Io(err)),
    }
}

fn safe_model_dir(root: &Path, id: ModelId) -> Result<PathBuf, ModelError> {
    let dir = model_dir_in(root, id);
    if !dir.starts_with(root) || dir == root {
        return Err(ModelError::InvalidPath(format!(
            "Resolved model dir '{}' escapes models root '{}'",
            dir.display(),
            root.display()
        )));
    }
    Ok(dir)
}

fn safe_child_path(dir: &Path, filename: &str) -> Result<PathBuf, ModelError> {
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') {
        return Err(ModelError::InvalidPath(format!(
            "Invalid model filename '{filename}'"
        )));
    }
    let rel = Path::new(filename);
    let mut comps = rel.components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(_)), None) => {}
        _ => {
            return Err(ModelError::InvalidPath(format!(
                "Path traversal rejected in model filename '{filename}'"
            )));
        }
    }
    let dest = dir.join(rel);
    if !dest.starts_with(dir) {
        return Err(ModelError::InvalidPath(format!(
            "Target '{}' escapes model directory '{}'",
            dest.display(),
            dir.display()
        )));
    }
    Ok(dest)
}

fn validate_file_contents(
    path: &Path,
    filename: &str,
    remove_on_failure: bool,
) -> Result<(), ModelError> {
    let meta = fs::metadata(path)?;
    if !meta.is_file() || meta.len() == 0 {
        if remove_on_failure {
            let _ = fs::remove_file(path);
        }
        return Err(ModelError::EmptyOrCorruptFile {
            filename: filename.to_owned(),
        });
    }

    // Reject HTML error pages mistakenly saved in place of binary ONNX graphs/weights.
    let mut file = File::open(path)?;
    let mut head = [0u8; 64];
    let n = file.read(&mut head)?;
    let prefix = String::from_utf8_lossy(&head[..n])
        .trim_start()
        .to_ascii_lowercase();
    if prefix.starts_with("<!doctype html") || prefix.starts_with("<html") {
        if remove_on_failure {
            let _ = fs::remove_file(path);
        }
        return Err(ModelError::EmptyOrCorruptFile {
            filename: filename.to_owned(),
        });
    }

    Ok(())
}
