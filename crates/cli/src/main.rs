//! `rd` — Agent-first, non-destructive RAW & photo editing CLI for Rune Darkroom.
//!
//! Designed for both AI agents (auto-JSON/NDJSON when piped, deterministic exit codes,
//! self-describing `rd schema` / `rd cli search`, quantitative `analyze` & CIEDE2000 `diff`)
//! and humans (crisp TTY summaries, atomic `.rune-edit.json` sidecars, batch export).

mod commands;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

/// Deterministic exit codes aligned with POSIX & `clap` conventions.
pub mod exit_code {
    pub const OK: u8 = 0;
    pub const PARTIAL_OR_ASSERT: u8 = 1;
    pub const VALIDATION: u8 = 2;
    pub const NOT_FOUND: u8 = 3;
    pub const DECODE: u8 = 4;
    pub const IO_OR_CONFLICT: u8 = 5;
    pub const GPU_UNAVAILABLE: u8 = 6;
}

/// Structured CLI error mapped directly to deterministic exit codes and JSON envelopes.
#[derive(Debug)]
pub struct CliError {
    pub code: &'static str,
    pub exit: u8,
    pub message: String,
    pub hint: Option<String>,
}

impl CliError {
    pub fn validation(msg: impl Into<String>) -> Self {
        Self {
            code: "VALIDATION_ERROR",
            exit: exit_code::VALIDATION,
            message: msg.into(),
            hint: Some("Run `rd schema recipe` or `rd cli search <term>` to inspect valid fields and bounds.".to_owned()),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: "INPUT_NOT_FOUND",
            exit: exit_code::NOT_FOUND,
            message: msg.into(),
            hint: None,
        }
    }

    pub fn decode(msg: impl Into<String>) -> Self {
        Self {
            code: "DECODE_FAILED",
            exit: exit_code::DECODE,
            message: msg.into(),
            hint: None,
        }
    }

    pub fn io(msg: impl Into<String>) -> Self {
        Self {
            code: "IO_ERROR",
            exit: exit_code::IO_OR_CONFLICT,
            message: msg.into(),
            hint: None,
        }
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self {
            code: "SIDECAR_CONFLICT",
            exit: exit_code::IO_OR_CONFLICT,
            message: msg.into(),
            hint: Some("Re-run with `--force` to overwrite concurrent sidecar changes.".to_owned()),
        }
    }

    pub fn assertion(msg: impl Into<String>) -> Self {
        Self {
            code: "ASSERTION_FAILED",
            exit: exit_code::PARTIAL_OR_ASSERT,
            message: msg.into(),
            hint: None,
        }
    }
}

impl From<rune_editor_core::CoreError> for CliError {
    fn from(err: rune_editor_core::CoreError) -> Self {
        match err {
            rune_editor_core::CoreError::Validation(m) => Self::validation(m),
            rune_editor_core::CoreError::UnknownParam(p) => Self {
                code: "UNKNOWN_PARAMETER",
                exit: exit_code::VALIDATION,
                message: format!("Unknown parameter '{p}'"),
                hint: Some("Run `rd cli search <query>` or `rd schema recipe` to list valid parameter names.".to_owned()),
            },
            rune_editor_core::CoreError::UnknownPreset(p) => Self {
                code: "UNKNOWN_PRESET",
                exit: exit_code::VALIDATION,
                message: format!("Unknown preset '{p}'"),
                hint: Some("Run `rd preset list` to view available built-in presets.".to_owned()),
            },
            rune_editor_core::CoreError::SidecarConflict { expected, actual } => {
                Self::conflict(format!(
                    "Sidecar changed concurrently (expected revision {expected}, found {actual})"
                ))
            }
            rune_editor_core::CoreError::CorruptSidecar { path, message } => Self::validation(
                format!(
                    "Refusing to overwrite corrupt sidecar '{}': {message}",
                    path.display()
                ),
            ),
            rune_editor_core::CoreError::Json(e) => Self::validation(format!("Invalid JSON: {e}")),
            rune_editor_core::CoreError::Io(e) => Self::io(format!("I/O error: {e}")),
        }
    }
}

impl From<rune_editor_decode::DecodeError> for CliError {
    fn from(err: rune_editor_decode::DecodeError) -> Self {
        match &err {
            rune_editor_decode::DecodeError::Io { source, .. }
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Self::not_found(err.to_string())
            }
            rune_editor_decode::DecodeError::Io { .. } => Self::io(err.to_string()),
            rune_editor_decode::DecodeError::SourceOverwrite(_)
            | rune_editor_decode::DecodeError::InvalidPath(_)
            | rune_editor_decode::DecodeError::DestinationExists(_) => {
                Self::validation(err.to_string())
            }
            rune_editor_decode::DecodeError::DuplicateDestination(_) => {
                let mut e = Self::validation(err.to_string());
                e.code = "DUPLICATE_OUTPUT_PATH";
                e
            }
            _ => Self::decode(err.to_string()),
        }
    }
}

impl From<rune_editor_decode::ModelError> for CliError {
    fn from(err: rune_editor_decode::ModelError) -> Self {
        match err {
            rune_editor_decode::ModelError::LicenseRequired {
                model_id,
                license,
                license_url,
            } => Self {
                code: "LICENSE_REQUIRED",
                exit: exit_code::VALIDATION,
                message: format!(
                    "Model '{}' requires accepting the {license} ({license_url}) before downloading or installing.",
                    model_id.id()
                ),
                hint: Some(format!(
                    "Review {license} at {license_url} and re-run with `--accept-license`."
                )),
            },
            rune_editor_decode::ModelError::MissingLocalFile { .. } => {
                Self::not_found(err.to_string())
            }
            rune_editor_decode::ModelError::InvalidPath(_)
            | rune_editor_decode::ModelError::EmptyOrCorruptFile { .. }
            | rune_editor_decode::ModelError::ChecksumMismatch { .. } => {
                Self::validation(err.to_string())
            }
            rune_editor_decode::ModelError::Busy(_) => Self::conflict(err.to_string()),
            rune_editor_decode::ModelError::Cancelled(_) => Self {
                code: "DOWNLOAD_CANCELLED",
                exit: exit_code::PARTIAL_OR_ASSERT,
                message: err.to_string(),
                hint: None,
            },
            rune_editor_decode::ModelError::DownloadFailed { .. }
            | rune_editor_decode::ModelError::Io(_) => Self::io(err.to_string()),
            rune_editor_decode::ModelError::Core(core_err) => core_err.into(),
        }
    }
}

impl From<rune_editor_decode::VisionError> for CliError {
    fn from(err: rune_editor_decode::VisionError) -> Self {
        match err {
            rune_editor_decode::VisionError::ModelNotInstalled(id) => Self {
                code: "MODEL_NOT_INSTALLED",
                exit: exit_code::NOT_FOUND,
                message: format!("Segmentation model '{}' is not installed.", id.id()),
                hint: Some(if id.is_sam3() {
                    format!(
                        "Install it with `rd model setup {} --accept-license` (requires {}), or use `--engine auto`.",
                        id.id(),
                        id.spec().license
                    )
                } else {
                    format!(
                        "Install it with `rd model setup {}`, or use `--engine auto`.",
                        id.id()
                    )
                }),
            },
            rune_editor_decode::VisionError::BackendUnavailable(msg) => Self {
                code: "BACKEND_UNAVAILABLE",
                exit: exit_code::VALIDATION,
                message: format!("Segmentation backend unavailable: {msg}"),
                hint: Some("Run `rd model status` or re-run with `--engine auto`.".to_owned()),
            },
            rune_editor_decode::VisionError::EmptyImage
            | rune_editor_decode::VisionError::InferenceFailed(_) => Self::decode(err.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Compact JSON/NDJSON when stdout is piped; readable text on a TTY.
    #[default]
    Auto,
    Json,
    Ndjson,
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedFormat {
    Json,
    Ndjson,
    Text,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum BackendMode {
    #[default]
    Auto,
    Gpu,
    Cpu,
}

/// Global runtime options passed into command handlers.
#[derive(Clone, Debug)]
pub struct RuntimeOpts {
    pub format: ResolvedFormat,
    pub dry_run: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub backend: BackendMode,
}

#[derive(Parser, Debug)]
#[command(
    name = "rd",
    version,
    about = "Rune Darkroom CLI — agent-first, non-destructive RAW & photo editor",
    long_about = "Agent-first, non-destructive RAW & photo editing CLI sharing Rune Darkroom's scene-referred linear engine and `.rune-edit.json` sidecar format."
)]
pub struct Cli {
    /// Output format (`auto` emits JSON/NDJSON when piped and readable text on a TTY).
    #[arg(long, global = true, default_value = "auto", value_enum)]
    pub format: OutputFormat,

    /// Shorthand for `--format json`.
    #[arg(long, global = true)]
    pub json: bool,

    /// Shorthand for `--format ndjson`.
    #[arg(long, global = true)]
    pub ndjson: bool,

    /// Simulate mutations without writing `.rune-edit.json` sidecars or exported images.
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Suppress non-essential status text.
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Include verbose fields in output payloads.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Compute backend selection (`auto`, `gpu`, or `cpu`).
    #[arg(long, global = true, default_value = "auto", value_enum)]
    pub backend: BackendMode,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print machine-readable JSON schemas, slider bounds, and valid enums.
    Schema {
        /// Target schema: `recipe`, `patch`, `batch`, `commands`, or `all`.
        #[arg(default_value = "all")]
        target: String,
    },
    /// Report platform decode tier, supported extensions, export formats, and presets.
    Capabilities,
    /// Search subcommands, recipe parameters, and presets by keyword.
    Cli {
        #[command(subcommand)]
        sub: CliSearchCmd,
    },
    /// Fast read-only inspection of photo EXIF metadata and `.rune-edit.json` sidecar state.
    Inspect(InspectArgs),
    /// Perceptual tone, color cast, skin-tone, palette, sharpness & auto-suggestion analysis.
    Analyze(AnalyzeArgs),
    /// Non-destructively edit one or more photos by updating `.rune-edit.json` sidecars.
    Edit(Box<EditArgs>),
    /// Batch-update star ratings, Pick/Reject cull flags, and 7 Color Tags without touching develop recipes.
    Cull(CullArgs),
    /// Manage non-destructive Virtual Variants (`list`, `new`, `clone`, `select`, `delete`).
    Variant {
        #[command(subcommand)]
        sub: VariantCmd,
    },
    /// Manage reusable User Styles and additive Style Stacking (`list`, `save`, `apply`, `delete`).
    Style {
        #[command(subcommand)]
        sub: StyleCmd,
    },
    /// Capture One-style Selective Adjustments Clipboard: copy selected or adjusted tool groups across photos.
    CopyLook(CopyLookArgs),
    /// Manage non-destructive Adjustment Layers, Combine Masks (`Add`/`Subtract`/`Intersect`), AI Subject/Background, and AI Select masks.
    Layer {
        #[command(subcommand)]
        sub: LayerCmd,
    },
    /// Manage AI segmentation engines and downloadable ONNX models (Apple Vision ANE, SAM 2.1, SAM 3).
    Model {
        #[command(subcommand)]
        sub: ModelCmd,
    },
    /// Manage non-destructive Spot Heal & Clone retouching stamps.
    Heal {
        #[command(subcommand)]
        sub: HealCmd,
    },
    /// Scan and filter a photo folder or session catalog.
    Session {
        #[command(subcommand)]
        sub: SessionCmd,
    },
    /// List, inspect, apply, or save reusable develop style presets.
    Preset {
        #[command(subcommand)]
        sub: PresetCmd,
    },
    /// List, import (`.dcp`/`.icc`/`.icm`/`.cube`), or delete Camera Profiles and Film Simulations.
    Profile {
        #[command(subcommand)]
        sub: ProfileCmd,
    },
    /// Render a fast vision-agent-optimized preview JPEG/PNG/WebP (default 1280px q=85).
    Preview(PreviewArgs),
    /// Compute deterministic CIEDE2000, exposure, clipping, and recipe diffs (with `--assert`).
    Diff(DiffArgs),
    /// Develop and export one or more photos (or folders) to JPEG, PNG8, PNG16, TIFF16, or WebP.
    Export(ExportArgs),
    /// Execute a declarative JSON batch job manifest (conditional rules + multi-target export).
    Batch {
        #[command(subcommand)]
        sub: BatchCmd,
    },
    /// Launch Rune Darkroom GUI on a photo or folder for interactive review.
    Open {
        /// Optional photo or folder path to open.
        path: Option<PathBuf>,
    },
    /// Generate shell completion scripts (`zsh`, `bash`, `fish`).
    Completions {
        /// Shell name (`zsh`, `bash`, `fish`).
        shell: clap_complete::Shell,
    },
    /// Run the local MCP 2025-11-25 server (stdio by default, or `--http` on 127.0.0.1).
    Mcp {
        /// Serve Streamable HTTP (`http://127.0.0.1:<port>/mcp`) instead of stdio.
        #[arg(long)]
        http: bool,
        /// Localhost port when `--http` is enabled (default `37420`).
        #[arg(long, default_value_t = rune_editor_mcp::DEFAULT_MCP_PORT)]
        port: u16,
    },
}

#[derive(Subcommand, Debug)]
pub enum CliSearchCmd {
    /// Search commands, parameters, and presets matching a query string.
    Search { query: String },
}

#[derive(Args, Debug)]
pub struct InspectArgs {
    /// Photo file path to inspect.
    pub path: PathBuf,
    /// Comma-separated extra sections: `full_recipe`, `history`, `histogram`.
    #[arg(long, value_delimiter = ',')]
    pub include: Vec<String>,
}

#[derive(Args, Debug)]
pub struct AnalyzeArgs {
    /// One or more photo files or folders to analyze.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    /// Recurse into subdirectories when folders are passed.
    #[arg(short, long)]
    pub recursive: bool,
    /// Proxy longest edge for tonal/color evaluation (default 320px).
    #[arg(long, default_value_t = 320)]
    pub proxy_edge: u32,
    /// Detail proxy longest edge for sharpness/focus scoring (default 1024px; 0 disables).
    #[arg(long, default_value_t = 1024)]
    pub detail_edge: u32,
    /// Skip detail sharpness pass for ultra-fast tonal triage.
    #[arg(long)]
    pub fast: bool,
    /// Probe a normalized pixel coordinate `x,y` (e.g. `0.5,0.4`) for RGB/HSL/Lab + WB pick.
    #[arg(long)]
    pub sample: Option<String>,
    /// Include EXIF and sidecar recipe summary in output.
    #[arg(long, value_delimiter = ',')]
    pub include: Vec<String>,
}

#[derive(Args, Debug)]
pub struct EditArgs {
    /// One or more photo files or folders to edit non-destructively.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    /// Recurse into subdirectories when folders are passed.
    #[arg(short, long)]
    pub recursive: bool,
    /// Only edit photos currently flagged as Pick.
    #[arg(long)]
    pub picks: bool,
    /// Only edit photos with star rating >= N.
    #[arg(long)]
    pub min_rating: Option<u8>,
    /// Set or nudge any registered parameter: `key=val`, `key+=val`, `key-=val`.
    #[arg(long = "set", value_name = "EXPR")]
    pub sets: Vec<String>,
    /// Apply a built-in preset ID or path to a `.json` preset file.
    #[arg(long)]
    pub preset: Option<String>,
    /// Blend strength `0.0..=1.0` when applying `--preset` (default 1.0).
    #[arg(long, default_value_t = 1.0)]
    pub mix: f32,
    /// Auto-adjustments (`wb`, `tone`, `levels`, `all`).
    #[arg(long, value_delimiter = ',')]
    pub auto: Vec<String>,
    /// Pick neutral white balance at normalized coordinates `x,y` (0..1).
    #[arg(long)]
    pub wb_pick: Option<String>,
    /// White balance illuminant preset (`as_shot`, `daylight`, `cloudy`, `shade`, `tungsten`, `fluorescent`, `flash`, `custom`).
    #[arg(long)]
    pub wb_preset: Option<String>,
    /// Camera profile or film simulation ID (`auto`, `pro_standard`, `classic_chrome`, `user:<slug>`, etc.).
    #[arg(long)]
    pub profile: Option<String>,
    /// Camera profile blend strength `0.0..=1.0` (default `1.0`).
    #[arg(long)]
    pub profile_strength: Option<f32>,
    /// Base Characteristics film curve (`film_standard`, `film_extra_shadow`, `film_high_contrast`, `linear_response`).
    #[arg(long)]
    pub film_curve: Option<String>,
    /// Processing mode (`photography` or `film_negative`).
    #[arg(long)]
    pub processing_mode: Option<String>,
    /// Sample raw pre-grade linear pixel at normalized coordinates `x,y` (0..1) to neutralize Film Negative base mask and enable Film Negative mode.
    #[arg(long, value_name = "X,Y")]
    pub pick_film_base: Option<String>,
    /// Sample linear pixel at normalized coordinates `x,y` (0..1) to set and enable Skin Tone target.
    #[arg(long, value_name = "X,Y")]
    pub pick_skin: Option<String>,
    /// Sample linear pixel for Advanced Color Editor slice `0..=3` (`idx:x,y`, e.g. `0:0.5,0.4`).
    #[arg(long = "pick-color-slice", value_name = "IDX:X,Y")]
    pub pick_color_slices: Vec<String>,
    /// Vignetting geometry method (`elliptic_on_crop`, `circular_on_crop`, `circular`).
    #[arg(long)]
    pub vignette_method: Option<String>,
    /// Copy recipe from another photo's sidecar (combine with `--only`).
    #[arg(long)]
    pub copy_from: Option<PathBuf>,
    /// Restrict `--copy-from` or `--preset` to comma-separated tool groups (`wb,tone,film_negative,color,hsl,advanced_color,skin_tone,curves,detail,lens,geometry,layers`).
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// Apply a sparse `RecipePatch` JSON string or `-` to read from stdin.
    #[arg(long)]
    pub patch: Option<String>,
    /// Reset a tool group (`all`, `tone`, `film_negative`, `wb`, `color`, `wheels`, `hsl`, `advanced_color`, `skin_tone`, `levels`, `curves`, `detail`, `bw`, `effects`, `lens`, `geometry`, `layers`).
    #[arg(long)]
    pub reset: Option<String>,
    /// Shorthand scalar flags (accept exact value or signed `+=` / `-=` via `--set`).
    #[arg(long, allow_hyphen_values = true)]
    pub exposure: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub contrast: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub highlights: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub shadows: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub temp: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub tint: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub vibrance: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub saturation: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub clarity: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub structure: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub dehaze: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub sharpen: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub denoise: Option<String>,
    /// Crop aspect ratio preset (`free`, `original`, `1:1`, `4:3`, `3:2`, `16:9`, `5:4`, `4:5`).
    #[arg(long)]
    pub aspect: Option<String>,
    /// Provenance actor label recorded in `.rune-edit.json` history (default `"cli"`).
    #[arg(long, default_value = "cli")]
    pub agent: String,
    /// Ignore optimistic concurrency revision conflicts.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct CullArgs {
    /// Photo paths or folders to cull (optional when `--from-stdin` is set).
    pub paths: Vec<PathBuf>,
    /// Read photo paths one per line from stdin.
    #[arg(long)]
    pub from_stdin: bool,
    /// Set star rating `0..=5`.
    #[arg(long)]
    pub rate: Option<u8>,
    /// Flag targets as Pick.
    #[arg(long, conflicts_with_all = ["reject", "unflag"])]
    pub pick: bool,
    /// Flag targets as Reject.
    #[arg(long, conflicts_with_all = ["pick", "unflag"])]
    pub reject: bool,
    /// Clear Pick/Reject flag.
    #[arg(long, conflicts_with_all = ["pick", "reject"])]
    pub unflag: bool,
    /// Set Capture One Color Tag (`none`, `red`, `orange`, `yellow`, `green`, `blue`, `purple`, `pink`).
    #[arg(long)]
    pub color_tag: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum VariantCmd {
    /// List all virtual variants on a photo.
    List { photo: PathBuf },
    /// Create a new neutral virtual variant and select it.
    New {
        photo: PathBuf,
        #[arg(long)]
        label: Option<String>,
    },
    /// Clone the currently active virtual variant and select the clone.
    Clone {
        photo: PathBuf,
        #[arg(long)]
        label: Option<String>,
    },
    /// Select a virtual variant by zero-based `--index`.
    Select {
        photo: PathBuf,
        #[arg(long)]
        index: usize,
    },
    /// Delete a virtual variant by zero-based `--index` (cannot delete the last remaining variant).
    Delete {
        photo: PathBuf,
        #[arg(long)]
        index: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum StyleCmd {
    /// List all saved User Styles.
    List {
        /// Optional custom styles directory (defaults to `~/.config/rune-darkroom/styles`).
        #[arg(long)]
        styles_dir: Option<PathBuf>,
    },
    /// Save a photo's develop recipe (or selected/adjusted groups) as a reusable User Style.
    Save {
        photo: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Optional comma-separated tool groups to capture (`wb,tone,levels,curves,wheels,color,skin_tone,bw,detail,effects,lens,geometry`).
        #[arg(long, value_delimiter = ',')]
        groups: Vec<String>,
        /// Automatically capture only tool groups modified from defaults.
        #[arg(long)]
        autoselect_adjusted: bool,
        #[arg(long)]
        styles_dir: Option<PathBuf>,
    },
    /// Apply or stack a saved User Style onto one or more photos.
    Apply {
        name: String,
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Stack style additively onto existing adjustments instead of replacing.
        #[arg(long)]
        stack: bool,
        /// Blend strength in `0.0..=1.0` (default `1.0`).
        #[arg(long, default_value_t = 1.0)]
        strength: f32,
        #[arg(long)]
        styles_dir: Option<PathBuf>,
    },
    /// Delete a saved User Style by name or slug.
    Delete {
        name: String,
        #[arg(long)]
        styles_dir: Option<PathBuf>,
    },
}

#[derive(Args, Debug)]
pub struct CopyLookArgs {
    /// Source photo whose sidecar adjustments will be copied.
    pub src: PathBuf,
    /// One or more destination photos or folders to apply adjustments to.
    #[arg(required = true)]
    pub dst: Vec<PathBuf>,
    /// Comma-separated tool groups (`wb,tone,levels,curves,wheels,color,skin_tone,bw,detail,effects,lens,geometry`).
    #[arg(long, value_delimiter = ',')]
    pub groups: Vec<String>,
    /// Automatically select only tool groups modified on `src` (Capture One Autoselect Adjusted).
    #[arg(long)]
    pub autoselect_adjusted: bool,
    /// Recurse into subdirectories when folders are passed in `dst`.
    #[arg(short, long)]
    pub recursive: bool,
}

#[derive(Subcommand, Debug)]
pub enum LayerCmd {
    /// List all adjustment layers and their masks on a photo.
    List { photo: PathBuf },
    /// Add a new adjustment layer (empty, filled, linear, radial, luma, ai-subject, ai-background, or ai-select).
    Add {
        photo: PathBuf,
        #[arg(long)]
        name: String,
        /// Initial mask kind (`empty`, `filled`, `linear`, `radial`, `luma`, `ai-subject`, `ai-background`, `ai-select`).
        #[arg(long, default_value = "empty")]
        kind: String,
        /// Layer opacity in `0.0..=1.0` (default `1.0`).
        #[arg(long, default_value_t = 1.0)]
        opacity: f32,
        /// Local adjustment assignments: `key=val`, `key+=delta`, `key-=delta` (`exposure`, `contrast`, `brightness`, `highlights`, `shadows`, `temperature`, `tint`, `saturation`, `clarity`, `dehaze`, `sharpen`).
        #[arg(long = "set", value_name = "EXPR")]
        sets: Vec<String>,
        /// Segmentation engine override for AI masks (`auto`, `apple-vision`, `sam2.1`, `sam3`, `heuristic`).
        #[arg(long)]
        engine: Option<String>,
        /// Repeatable interactive prompt point `x,y[,+|-]` in normalized `0..=1` coords for `--kind ai-select` (default `+`).
        #[arg(long = "point", value_name = "X,Y[,+|-]")]
        points: Vec<String>,
        /// Interactive bounding box `x0,y0,x1,y1` in normalized `0..=1` coords for `--kind ai-select`.
        #[arg(long = "box", value_name = "X0,Y0,X1,Y1")]
        bbox: Option<String>,
        /// Invert the generated AI selection mask (`--kind ai-select`).
        #[arg(long, default_value_t = false)]
        invert: bool,
    },
    /// Update an existing adjustment layer's metadata or local adjustments.
    Update {
        photo: PathBuf,
        /// Zero-based layer index or layer ID.
        #[arg(long)]
        index: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        opacity: Option<f32>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        inverted: Option<bool>,
        /// Local adjustment assignments (`key=val`, `key+=delta`, `key-=delta`).
        #[arg(long = "set", value_name = "EXPR")]
        sets: Vec<String>,
    },
    /// Add a composable mask (`add`, `subtract`, or `intersect`) onto an adjustment layer.
    MaskAdd {
        photo: PathBuf,
        /// Zero-based layer index or layer ID.
        #[arg(long)]
        index: String,
        /// Mask kind (`linear`, `radial`, `luma`, `filled`, `ai-subject`, `ai-background`, `ai-select`).
        #[arg(long)]
        kind: String,
        /// Combine operation (`add`, `subtract`, `intersect`).
        #[arg(long, default_value = "add")]
        op: String,
        /// Linear gradient start point `x,y` (normalized `0..=1`).
        #[arg(long)]
        start: Option<String>,
        /// Linear gradient end point `x,y` (normalized `0..=1`).
        #[arg(long)]
        end: Option<String>,
        /// Radial gradient center `x,y` (normalized `0..=1`).
        #[arg(long)]
        center: Option<String>,
        /// Radial gradient semi-axes `rx,ry` (or single `r`).
        #[arg(long)]
        radius: Option<String>,
        /// Radial gradient rotation angle in degrees.
        #[arg(long, allow_hyphen_values = true, default_value_t = 0.0)]
        angle: f32,
        /// Gradient feather softness (`0..=1`).
        #[arg(long)]
        feather: Option<f32>,
        /// Luma range low luminance (`0..=1`).
        #[arg(long, default_value_t = 0.0)]
        low: f32,
        /// Luma range high luminance (`0..=1`).
        #[arg(long, default_value_t = 1.0)]
        high: f32,
        /// Luma range lower feather falloff (`0..=1`).
        #[arg(long, default_value_t = 0.1)]
        low_falloff: f32,
        /// Luma range upper feather falloff (`0..=1`).
        #[arg(long, default_value_t = 0.1)]
        high_falloff: f32,
        /// Invert this mask shape.
        #[arg(long, default_value_t = false)]
        inverted: bool,
        /// Segmentation engine override for AI masks (`auto`, `apple-vision`, `sam2.1`, `sam3`, `heuristic`).
        #[arg(long)]
        engine: Option<String>,
        /// Repeatable interactive prompt point `x,y[,+|-]` in normalized `0..=1` coords for `--kind ai-select` (default `+`).
        #[arg(long = "point", value_name = "X,Y[,+|-]")]
        points: Vec<String>,
        /// Interactive bounding box `x0,y0,x1,y1` in normalized `0..=1` coords for `--kind ai-select`.
        #[arg(long = "box", value_name = "X0,Y0,X1,Y1")]
        bbox: Option<String>,
        /// Invert the generated AI selection mask (`--kind ai-select`).
        #[arg(long, default_value_t = false)]
        invert: bool,
    },
    /// Clear all masks from an adjustment layer.
    MaskClear {
        photo: PathBuf,
        /// Zero-based layer index or layer ID.
        #[arg(long)]
        index: String,
    },
    /// Remove an adjustment layer by zero-based index or layer ID.
    Remove {
        photo: PathBuf,
        /// Zero-based layer index or layer ID.
        #[arg(long)]
        index: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ModelCmd {
    /// List built-in Apple Vision ANE and all downloadable ONNX segmentation models with install & license status.
    List,
    /// Show active engine configuration, effective Subject & Interactive engines, models directory, and disk usage.
    Status,
    /// Download or locally import ONNX weights for a segmentation model (`sam2.1-hiera-tiny`, `sam3-tracker-q4f16`, `sam3-tracker-fp16`).
    #[command(visible_alias = "download")]
    Setup {
        /// Model ID or alias (e.g. `sam2.1`, `sam3`, `sam3-tracker-q4f16`, `sam3-tracker-fp16`).
        model: String,
        /// Explicitly accept the model's license (required for Meta SAM License models such as SAM 3).
        #[arg(long)]
        accept_license: bool,
        /// Offline import from a local directory containing the model's `.onnx` / `.onnx_data` files instead of downloading.
        #[arg(long)]
        from_dir: Option<PathBuf>,
    },
    /// Select and persist the active segmentation engine (`auto`, `apple-vision`, `sam2.1`, `sam3`, `heuristic`).
    Use {
        /// Engine identifier (`auto`, `apple-vision`, `sam2.1`, `sam3`, `heuristic`).
        engine: String,
    },
    /// Remove installed ONNX weights for a segmentation model from disk.
    Remove {
        /// Model ID or alias to remove.
        model: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum HealCmd {
    /// List all Spot Heal & Clone stamps on a photo.
    List { photo: PathBuf },
    /// Add a Spot Heal or Clone stamp at normalized `--target x,y`.
    Add {
        photo: PathBuf,
        /// Target patch center `x,y` in `0..=1`.
        #[arg(long)]
        target: String,
        /// Optional source patch center `x,y` in `0..=1` (auto-selected when omitted).
        #[arg(long)]
        source: Option<String>,
        /// Normalized patch radius (`0.002..=0.25`, default `0.03`).
        #[arg(long, default_value_t = 0.03)]
        radius: f32,
        /// Edge feather softness (`0.0..=1.0`, default `0.4`).
        #[arg(long, default_value_t = 0.4)]
        feather: f32,
        /// Stamp opacity (`0.0..=1.0`, default `1.0`).
        #[arg(long, default_value_t = 1.0)]
        opacity: f32,
        /// Retouch mode (`heal` or `clone`).
        #[arg(long, default_value = "heal")]
        mode: String,
    },
    /// Remove a Spot Heal/Clone stamp by zero-based index or ID.
    Remove {
        photo: PathBuf,
        /// Zero-based spot index or spot ID.
        #[arg(long)]
        index: String,
    },
    /// Remove all Spot Heal & Clone stamps from a photo.
    Clear { photo: PathBuf },
}

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// Scan a folder and report culling counts, EXIF, and sidecar status.
    Scan {
        /// Folder path to scan.
        dir: PathBuf,
        #[arg(short, long)]
        recursive: bool,
        #[arg(long)]
        picks: bool,
        #[arg(long)]
        rejects: bool,
        #[arg(long)]
        min_rating: Option<u8>,
        /// Filter by Capture One Color Tag (`none`, `red`, `orange`, `yellow`, `green`, `blue`, `purple`, `pink`).
        #[arg(long)]
        color_tag: Option<String>,
        /// Case-insensitive substring search across filename, camera, lens, preset, and color tag.
        #[arg(long)]
        search: Option<String>,
        /// Comma-separated file extensions to include (e.g. `arw,dng`).
        #[arg(long, value_delimiter = ',')]
        ext: Vec<String>,
    },
    /// Initialize a Capture One-style 4-folder Session (`Capture/`, `Selects/`, `Output/`, `Trash/`).
    Init {
        /// Root directory for the Capture One session.
        dir: PathBuf,
    },
    /// Watch a hot folder for newly arriving photos and auto-ingest them (optionally applying a Style).
    Watch {
        /// Folder path to watch for incoming photos.
        dir: PathBuf,
        /// Optional built-in preset ID or User Style name to apply on ingest.
        #[arg(long)]
        style: Option<String>,
        /// Run a deterministic single ingest pass over stable photos in `dir` and exit.
        #[arg(long)]
        once: bool,
        /// Poll interval in milliseconds (default `500`).
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
}

#[derive(Subcommand, Debug)]
pub enum PresetCmd {
    /// List all built-in presets and their non-default parameter summaries.
    List,
    /// Print the full `DevelopRecipe` JSON of a preset ID or file.
    Show { id: String },
    /// Apply a preset to one or more photos or folders.
    Apply {
        id: String,
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long, default_value_t = 1.0)]
        mix: f32,
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
    },
    /// Extract a photo's current recipe into a standalone reusable JSON preset file.
    Save {
        photo: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProfileCmd {
    /// List all bundled Camera Calibration, Film Simulation, and imported User Profiles.
    List {
        /// Optional EXIF camera model to highlight camera-matched profiles (e.g. `"ILCE-7RM5"` or `"X-T5"`).
        #[arg(long)]
        camera: Option<String>,
        /// Optional EXIF camera make to pair with `--camera` (e.g. `"Sony"`, `"Fujifilm"`, `"Apple"`).
        #[arg(long)]
        make: Option<String>,
        /// Optional custom user profiles directory (defaults to `~/.config/rune-darkroom/profiles`).
        #[arg(long)]
        profiles_dir: Option<PathBuf>,
    },
    /// Import an external `.dcp`, `.icc`, `.icm`, or `.cube` camera profile into the user store.
    Import {
        /// Path to a `.dcp`, `.icc`, `.icm`, or `.cube` profile file.
        file: PathBuf,
        /// Optional profile name / slug override (defaults to embedded profile name or file stem).
        #[arg(long)]
        name: Option<String>,
        /// Overwrite an existing user profile with the same slug.
        #[arg(long)]
        overwrite: bool,
        /// Optional custom user profiles directory (defaults to `~/.config/rune-darkroom/profiles`).
        #[arg(long)]
        profiles_dir: Option<PathBuf>,
    },
    /// Delete an imported user camera profile by slug (or `user:<slug>`).
    Delete {
        /// User profile slug (or `user:<slug>`).
        slug: String,
        /// Optional custom user profiles directory (defaults to `~/.config/rune-darkroom/profiles`).
        #[arg(long)]
        profiles_dir: Option<PathBuf>,
    },
}

#[derive(Args, Debug)]
pub struct PreviewArgs {
    /// Photo file to render a preview for.
    pub photo: PathBuf,
    /// Output image path (e.g. `/tmp/preview.jpg`).
    #[arg(short, long)]
    pub out: PathBuf,
    /// Longest edge in pixels (default `1280`, optimal for vision models).
    #[arg(long, default_value_t = 1280)]
    pub max_edge: u32,
    /// JPEG quality `1..=100` (default `85`).
    #[arg(long, default_value_t = 85)]
    pub quality: u8,
    /// Ignore persistent crop/geometry when rendering preview.
    #[arg(long)]
    pub no_crop: bool,
    /// Allow overwriting an existing file at `--out` (never permits overwriting source/RAW files).
    #[arg(long)]
    pub overwrite: bool,
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Photo file whose current sidecar recipe is evaluated.
    pub photo: PathBuf,
    /// Optional second photo whose sidecar is used as the baseline instead of `neutral`.
    pub baseline: Option<PathBuf>,
    /// Compare against a built-in preset ID instead of `neutral`.
    #[arg(long)]
    pub against_preset: Option<String>,
    /// Proxy longest edge for evaluation (default `320`).
    #[arg(long, default_value_t = 320)]
    pub proxy_edge: u32,
    /// Verification guardrails (e.g. `max_highlight_clip_pct=0.5,max_skin_de2000=6.0,max_mean_de2000=12.0`).
    #[arg(long = "assert", value_delimiter = ',')]
    pub assertions: Vec<String>,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// One or more photo files or folders to export.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    /// Output file (for a single photo) or output directory (for batches).
    #[arg(short, long)]
    pub out: PathBuf,
    /// Output format: `jpeg`, `png8`, `png16`, `tiff16`, `webp`.
    #[arg(long = "to", default_value = "jpeg")]
    pub to: String,
    /// JPEG quality `1..=100` (default `92`).
    #[arg(long, default_value_t = 92)]
    pub quality: u8,
    /// Optional longest-edge cap in pixels (e.g. `2048` for web).
    #[arg(long)]
    pub max_edge: Option<u32>,
    /// Post-resize screen output sharpening `0.0..=1.0`.
    #[arg(long, default_value_t = 0.0)]
    pub output_sharpen: f32,
    /// Ignore persistent crop/geometry transforms.
    #[arg(long)]
    pub no_crop: bool,
    /// Filename template when exporting into a directory (supports `{stem}`, `{ext}`).
    #[arg(long, default_value = "{stem}.{ext}")]
    pub naming: String,
    /// Capture One-style dynamic naming token template (e.g. `{ImageName}_{Recipe}_{ISO}`).
    #[arg(long)]
    pub naming_template: Option<String>,
    /// Optional anti-aliased text watermark rendered onto exported images.
    #[arg(long)]
    pub watermark: Option<String>,
    /// Text watermark opacity in `0.0..=1.0` (default `0.45`).
    #[arg(long, default_value_t = 0.45)]
    pub watermark_opacity: f32,
    /// Text watermark position (`bottom_right`, `bottom_left`, `top_right`, `top_left`, `center`).
    #[arg(long, default_value = "bottom_right")]
    pub watermark_position: String,
    /// Text watermark scale multiplier in `0.5..=3.0` (default `1.0`).
    #[arg(long, default_value_t = 1.0)]
    pub watermark_scale: f32,
    /// Strip camera EXIF/metadata from exported files.
    #[arg(long)]
    pub strip_metadata: bool,
    /// Output color space (`srgb` or `display-p3`).
    #[arg(long, default_value = "srgb")]
    pub color_space: String,
    /// Simultaneous Multi-Recipe export: comma-separated builtin slugs (`full_res_jpeg,web_2048px,archival_16_bit_tiff` or `all`), inline JSON array, or path to JSON recipe specs.
    #[arg(long)]
    pub recipes: Option<String>,
    /// Recurse into subdirectories.
    #[arg(short, long)]
    pub recursive: bool,
    /// Export only photos flagged as Pick.
    #[arg(long)]
    pub picks: bool,
    /// Export only photos with rating >= N.
    #[arg(long)]
    pub min_rating: Option<u8>,
    /// Skip writing if destination file already exists.
    #[arg(long)]
    pub skip_existing: bool,
    /// Allow overwriting existing non-source export files.
    #[arg(long)]
    pub overwrite: bool,
    /// Maximum concurrent image exports (default 2 for bounded RAM).
    #[arg(short, long, default_value_t = 2)]
    pub jobs: usize,
}

#[derive(Subcommand, Debug)]
pub enum BatchCmd {
    /// Execute a declarative JSON batch manifest file (or `-` for stdin).
    Run {
        /// Path to `BatchJob` JSON file, or `-` for stdin.
        manifest: String,
        /// Maximum concurrent image jobs (default 2).
        #[arg(short, long, default_value_t = 2)]
        jobs: usize,
    },
}

fn resolve_format(cli: &Cli, streaming_default: bool) -> ResolvedFormat {
    if cli.ndjson {
        return ResolvedFormat::Ndjson;
    }
    if cli.json {
        return ResolvedFormat::Json;
    }
    match cli.format {
        OutputFormat::Json => ResolvedFormat::Json,
        OutputFormat::Ndjson => ResolvedFormat::Ndjson,
        OutputFormat::Text => ResolvedFormat::Text,
        OutputFormat::Auto => {
            if std::io::stdout().is_terminal() {
                ResolvedFormat::Text
            } else if streaming_default {
                ResolvedFormat::Ndjson
            } else {
                ResolvedFormat::Json
            }
        }
    }
}

pub fn emit_ok(opts: &RuntimeOpts, command: &str, data: Value, human_text: &str) {
    match opts.format {
        ResolvedFormat::Json | ResolvedFormat::Ndjson => {
            let mut env = json!({
                "ok": true,
                "command": command,
                "schema_version": 1,
                "data": data,
            });
            if opts.dry_run
                && let Some(obj) = env.as_object_mut()
            {
                obj.insert("dry_run".to_owned(), json!(true));
            }
            println!("{env}");
        }
        ResolvedFormat::Text => {
            if !opts.quiet && !human_text.is_empty() {
                println!("{human_text}");
            }
        }
    }
}

pub fn emit_ndjson_line(value: &Value) {
    println!("{value}");
}

fn emit_error(opts: &RuntimeOpts, command: &str, err: &CliError) {
    match opts.format {
        ResolvedFormat::Json | ResolvedFormat::Ndjson => {
            let env = json!({
                "ok": false,
                "command": command,
                "error": {
                    "code": err.code,
                    "exit_code": err.exit,
                    "message": err.message,
                    "hint": err.hint,
                }
            });
            // Emit structured JSON error on stdout so agents capturing stdout always parse valid JSON.
            println!("{env}");
        }
        ResolvedFormat::Text => {
            eprintln!("error [{}]: {}", err.code, err.message);
            if let Some(hint) = &err.hint {
                eprintln!("hint: {hint}");
            }
        }
    }
}

fn command_name(cmd: &Command) -> &'static str {
    match cmd {
        Command::Schema { .. } => "schema",
        Command::Capabilities => "capabilities",
        Command::Cli { .. } => "cli.search",
        Command::Inspect(_) => "inspect",
        Command::Analyze(_) => "analyze",
        Command::Edit(_) => "edit",
        Command::Cull(_) => "cull",
        Command::Variant { .. } => "variant",
        Command::Style { .. } => "style",
        Command::CopyLook(_) => "copy-look",
        Command::Layer { .. } => "layer",
        Command::Model { .. } => "model",
        Command::Heal { .. } => "heal",
        Command::Session { .. } => "session.scan",
        Command::Preset { .. } => "preset",
        Command::Profile { .. } => "profile",
        Command::Preview(_) => "preview",
        Command::Diff(_) => "diff",
        Command::Export(_) => "export",
        Command::Batch { .. } => "batch.run",
        Command::Open { .. } => "open",
        Command::Completions { .. } => "completions",
        Command::Mcp { .. } => "mcp",
    }
}

fn is_streaming_command(cmd: &Command) -> bool {
    matches!(cmd, Command::Batch { .. })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cmd_label = command_name(&cli.command);
    let opts = RuntimeOpts {
        format: resolve_format(&cli, is_streaming_command(&cli.command)),
        dry_run: cli.dry_run,
        quiet: cli.quiet,
        verbose: cli.verbose,
        backend: cli.backend,
    };

    if matches!(opts.backend, BackendMode::Gpu)
        && matches!(
            cli.command,
            Command::Export(_) | Command::Preview(_) | Command::Batch { .. }
        )
    {
        let err = CliError {
            code: "GPU_BACKEND_UNAVAILABLE",
            exit: exit_code::GPU_UNAVAILABLE,
            message: "Headless GPU export backend is scheduled for Phase B; use `--backend auto` or `--backend cpu`.".to_owned(),
            hint: Some("Re-run with `--backend auto` to use the linear multi-core CPU develop pipeline.".to_owned()),
        };
        emit_error(&opts, cmd_label, &err);
        return ExitCode::from(err.exit);
    }

    match commands::dispatch(cli.command, &opts) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            emit_error(&opts, cmd_label, &err);
            ExitCode::from(err.exit)
        }
    }
}
