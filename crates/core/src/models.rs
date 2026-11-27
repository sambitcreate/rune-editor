//! AI segmentation engine selection, downloadable ONNX model registry (SAM 2.1 & SAM 3),
//! interactive prompt types, install inspection, and atomic configuration persistence.
//!
//! Strictly UI-, GPU-, and network-agnostic: download and inference live in downstream crates
//! while this module owns the canonical identifiers, Hugging Face file manifests, license metadata,
//! prompt contracts, and safe `~/.config/rune-darkroom/` storage paths.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    CoreError,
    session::{acquire_path_lock, atomic_write_bytes},
};

/// Environment variable overriding the default AI models directory (`~/.config/rune-darkroom/models`).
pub const RUNE_MODELS_DIR_ENV: &str = "RUNE_MODELS_DIR";

/// Maximum number of interactive prompt points accepted in a [`SmartMaskPrompt::Interactive`].
pub const MAX_PROMPT_POINTS: usize = 64;

const fn default_true() -> bool {
    true
}

const fn default_preferred_sam3_variant() -> ModelId {
    ModelId::Sam3TrackerQ4f16
}

/// Active segmentation engine selector shared across GUI, `rd` CLI, and MCP.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentationEngine {
    /// Automatically pick the strongest available backend:
    /// - Subject/Background: Apple Vision ANE (macOS) -> installed SAM 3 -> SAM 2.1 -> Heuristic.
    /// - Interactive (points/box): installed SAM 3 -> SAM 2.1 -> Apple Vision ANE -> Heuristic.
    #[default]
    Auto,
    /// macOS Apple Vision `VNGenerateForegroundInstanceMaskRequest` (built-in, zero download).
    AppleVision,
    /// Meta SAM 2.1 Hiera Tiny FP16 ONNX (~78 MB, Apache-2.0).
    Sam21Tiny,
    /// Meta SAM 3 Tracker ONNX (Q4F16 ~302 MB or FP16 ~947 MB, Meta SAM License).
    Sam3Tracker,
    /// Pure-Rust cross-platform perceptual heuristic fallback (zero download).
    Heuristic,
}

impl SegmentationEngine {
    /// All supported segmentation engines in canonical display order.
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::AppleVision,
        Self::Sam21Tiny,
        Self::Sam3Tracker,
        Self::Heuristic,
    ];

    /// Canonical snake_case identifier matching serde serialization and CLI/MCP flags.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AppleVision => "apple_vision",
            Self::Sam21Tiny => "sam21_tiny",
            Self::Sam3Tracker => "sam3_tracker",
            Self::Heuristic => "heuristic",
        }
    }

    /// Human-readable display label for UI pickers and status reports.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::AppleVision => "Apple Vision",
            Self::Sam21Tiny => "SAM 2.1 Tiny",
            Self::Sam3Tracker => "SAM 3 Tracker",
            Self::Heuristic => "Heuristic",
        }
    }

    /// Parses an engine identifier permissively (case-insensitive, accepts `-`/`_`/common aliases).
    pub fn from_str_loose(s: &str) -> Option<Self> {
        let norm = s.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        match norm.as_str() {
            "auto" | "automatic" | "default" => Some(Self::Auto),
            "apple_vision" | "applevision" | "vision" | "apple" => Some(Self::AppleVision),
            "sam21_tiny" | "sam2.1_tiny" | "sam2.1_hiera_tiny" | "sam21_hiera_tiny" | "sam2.1"
            | "sam21" | "sam2" => Some(Self::Sam21Tiny),
            "sam3_tracker" | "sam3_tracker_q4f16" | "sam3_tracker_fp16" | "sam3_q4f16"
            | "sam3_fp16" | "sam3" => Some(Self::Sam3Tracker),
            "heuristic" | "fallback" | "builtin" | "cpu" => Some(Self::Heuristic),
            _ => None,
        }
    }
}

impl fmt::Display for SegmentationEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Downloadable ONNX segmentation model identifier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelId {
    /// SAM 2.1 Hiera Tiny FP16 (`onnx-community/sam2.1-hiera-tiny-ONNX`, Apache-2.0, ~78 MB).
    #[serde(rename = "sam2.1-hiera-tiny")]
    Sam21HieraTinyFp16,
    /// SAM 3 Tracker Q4F16 (`onnx-community/sam3-tracker-ONNX`, Meta SAM License, ~302 MB).
    #[default]
    #[serde(rename = "sam3-tracker-q4f16")]
    Sam3TrackerQ4f16,
    /// SAM 3 Tracker FP16 (`onnx-community/sam3-tracker-ONNX`, Meta SAM License, ~947 MB).
    #[serde(rename = "sam3-tracker-fp16")]
    Sam3TrackerFp16,
}

impl ModelId {
    /// All downloadable models in canonical catalog order.
    pub const ALL: [Self; 3] = [
        Self::Sam21HieraTinyFp16,
        Self::Sam3TrackerQ4f16,
        Self::Sam3TrackerFp16,
    ];

    /// Canonical hyphenated model ID used on disk, in JSON, CLI, and MCP.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Sam21HieraTinyFp16 => "sam2.1-hiera-tiny",
            Self::Sam3TrackerQ4f16 => "sam3-tracker-q4f16",
            Self::Sam3TrackerFp16 => "sam3-tracker-fp16",
        }
    }

    /// Alias of [`Self::id`].
    pub const fn as_str(self) -> &'static str {
        self.id()
    }

    /// Human-readable label for UI & CLI output.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Sam21HieraTinyFp16 => "SAM 2.1 Hiera Tiny (FP16)",
            Self::Sam3TrackerQ4f16 => "SAM 3 Tracker (Q4F16)",
            Self::Sam3TrackerFp16 => "SAM 3 Tracker (FP16)",
        }
    }

    /// Short aliases accepted by CLI and MCP model setup commands.
    pub const fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Sam21HieraTinyFp16 => &["sam2.1", "sam2", "sam2.1-tiny"],
            Self::Sam3TrackerQ4f16 => &["sam3", "sam3-q4f16", "sam3-tracker"],
            Self::Sam3TrackerFp16 => &["sam3-fp16"],
        }
    }

    /// Segmentation engine powered by this downloadable model.
    pub const fn engine(self) -> SegmentationEngine {
        match self {
            Self::Sam21HieraTinyFp16 => SegmentationEngine::Sam21Tiny,
            Self::Sam3TrackerQ4f16 | Self::Sam3TrackerFp16 => SegmentationEngine::Sam3Tracker,
        }
    }

    /// Returns `true` for SAM 3 Tracker variants.
    pub const fn is_sam3(self) -> bool {
        matches!(self, Self::Sam3TrackerQ4f16 | Self::Sam3TrackerFp16)
    }

    /// Returns the static [`ModelSpec`] manifest for this model.
    pub const fn spec(self) -> &'static ModelSpec {
        model_spec(self)
    }

    /// Parses a model ID or alias permissively (case-insensitive, accepts `_` or `-`).
    pub fn from_str_loose(s: &str) -> Option<Self> {
        let norm = s.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        match norm.as_str() {
            "sam2.1-hiera-tiny"
            | "sam2.1-hiera-tiny-fp16"
            | "sam21-hiera-tiny"
            | "sam2.1"
            | "sam21"
            | "sam2"
            | "sam2.1-tiny"
            | "sam21-tiny" => Some(Self::Sam21HieraTinyFp16),
            "sam3-tracker-q4f16" | "sam3" | "sam3-q4f16" | "sam3-tracker" => {
                Some(Self::Sam3TrackerQ4f16)
            }
            "sam3-tracker-fp16" | "sam3-fp16" => Some(Self::Sam3TrackerFp16),
            _ => None,
        }
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// A single downloadable file belonging to a [`ModelSpec`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ModelFileSpec {
    /// Local filename inside [`model_dir`].
    pub filename: &'static str,
    /// Immutable commit-pinned Hugging Face `resolve/<commit>/` download URL.
    pub url: &'static str,
    /// Exact expected file size in bytes (matching Hugging Face LFS size).
    pub approx_bytes: u64,
    /// Verified lowercase hex SHA-256 digest of the file contents (Hugging Face LFS OID).
    pub sha256: &'static str,
}

/// Complete immutable manifest and preprocessing contract for a downloadable ONNX model.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ModelSpec {
    pub id: ModelId,
    pub slug: &'static str,
    pub display_name: &'static str,
    pub aliases: &'static [&'static str],
    pub engine: SegmentationEngine,
    pub hf_repo: &'static str,
    pub license: &'static str,
    pub license_url: &'static str,
    pub requires_license_acceptance: bool,
    pub files: &'static [ModelFileSpec],
    /// Square RGB input tensor resolution (`1024` for SAM 2.1, `1008` for SAM 3).
    pub input_size: u32,
    /// Per-channel RGB normalization mean (`(pixel - mean) / std`).
    pub norm_mean: [f32; 3],
    /// Per-channel RGB normalization standard deviation.
    pub norm_std: [f32; 3],
    /// Native decoder low-resolution mask logit side length (`256` for SAM 2.1, `288` for SAM 3).
    pub mask_size: u32,
    /// Filename of the ONNX vision encoder graph inside [`model_dir`].
    pub encoder_file: &'static str,
    /// Filename of the ONNX prompt encoder + mask decoder graph inside [`model_dir`].
    pub decoder_file: &'static str,
    /// Total approximate download size in bytes across all [`Self::files`].
    pub approx_bytes: u64,
}

const SAM21_HIERA_TINY_FILES: [ModelFileSpec; 4] = [
    ModelFileSpec {
        filename: "vision_encoder_fp16.onnx",
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/814a066640debee5a91e70aa401fb8e17e030503/onnx/vision_encoder_fp16.onnx",
        approx_bytes: 314_925,
        sha256: "7773e79d589dada8e1e630cc7eb18b17b2c5b4faeb1eaab5ebf9df618ecbd50a",
    },
    ModelFileSpec {
        filename: "vision_encoder_fp16.onnx_data",
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/814a066640debee5a91e70aa401fb8e17e030503/onnx/vision_encoder_fp16.onnx_data",
        approx_bytes: 67_005_504,
        sha256: "a4dd3759e9b6a476d991fb3493787992e78777e51462695e1bede71ae258103e",
    },
    ModelFileSpec {
        filename: "prompt_encoder_mask_decoder_fp16.onnx",
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/814a066640debee5a91e70aa401fb8e17e030503/onnx/prompt_encoder_mask_decoder_fp16.onnx",
        approx_bytes: 229_799,
        sha256: "00ac7200451f246e8307aa1a1648113f47dc45625738406403a0e6b86c69fc55",
    },
    ModelFileSpec {
        filename: "prompt_encoder_mask_decoder_fp16.onnx_data",
        url: "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/814a066640debee5a91e70aa401fb8e17e030503/onnx/prompt_encoder_mask_decoder_fp16.onnx_data",
        approx_bytes: 10_454_016,
        sha256: "027b457a53f5bd76bb401c59555448451d04b4eab6d2f6a6758406b40133ee2c",
    },
];

const SAM3_TRACKER_Q4F16_FILES: [ModelFileSpec; 4] = [
    ModelFileSpec {
        filename: "vision_encoder_q4f16.onnx",
        url: "https://huggingface.co/onnx-community/sam3-tracker-ONNX/resolve/429305c8a5b3de597243d919a07e4e6bdcd00ef7/onnx/vision_encoder_q4f16.onnx",
        approx_bytes: 1_307_816,
        sha256: "4b96732adab3d7d70ac0964ce540a61fad097fbc10c9b27612f47d676877cefc",
    },
    ModelFileSpec {
        filename: "vision_encoder_q4f16.onnx_data",
        url: "https://huggingface.co/onnx-community/sam3-tracker-ONNX/resolve/429305c8a5b3de597243d919a07e4e6bdcd00ef7/onnx/vision_encoder_q4f16.onnx_data",
        approx_bytes: 295_622_656,
        sha256: "d1d60808376dbd48e34cf89feaee708c13abed229f2398ab512149db34fdab07",
    },
    ModelFileSpec {
        filename: "prompt_encoder_mask_decoder_q4f16.onnx",
        url: "https://huggingface.co/onnx-community/sam3-tracker-ONNX/resolve/429305c8a5b3de597243d919a07e4e6bdcd00ef7/onnx/prompt_encoder_mask_decoder_q4f16.onnx",
        approx_bytes: 243_598,
        sha256: "6f77d5e2fb6fcb325297734b46cbb951534be08978599856d17c0030b51ea897",
    },
    ModelFileSpec {
        filename: "prompt_encoder_mask_decoder_q4f16.onnx_data",
        url: "https://huggingface.co/onnx-community/sam3-tracker-ONNX/resolve/429305c8a5b3de597243d919a07e4e6bdcd00ef7/onnx/prompt_encoder_mask_decoder_q4f16.onnx_data",
        approx_bytes: 5_121_024,
        sha256: "4c330254898062cce1ded5f2208e57b441df443886161af02b492b5a5ae2f6d0",
    },
];

const SAM3_TRACKER_FP16_FILES: [ModelFileSpec; 4] = [
    ModelFileSpec {
        filename: "vision_encoder_fp16.onnx",
        url: "https://huggingface.co/onnx-community/sam3-tracker-ONNX/resolve/429305c8a5b3de597243d919a07e4e6bdcd00ef7/onnx/vision_encoder_fp16.onnx",
        approx_bytes: 1_263_005,
        sha256: "f8c52be6de99124bb17f25792054406abc482ce3502166f948d5c5849cd14d02",
    },
    ModelFileSpec {
        filename: "vision_encoder_fp16.onnx_data",
        url: "https://huggingface.co/onnx-community/sam3-tracker-ONNX/resolve/429305c8a5b3de597243d919a07e4e6bdcd00ef7/onnx/vision_encoder_fp16.onnx_data",
        approx_bytes: 934_729_728,
        sha256: "4b021a4d3068c9e9153f2f2fcc9e6d280156228671c00ab6c2ddfa737da7f151",
    },
    ModelFileSpec {
        filename: "prompt_encoder_mask_decoder_fp16.onnx",
        url: "https://huggingface.co/onnx-community/sam3-tracker-ONNX/resolve/429305c8a5b3de597243d919a07e4e6bdcd00ef7/onnx/prompt_encoder_mask_decoder_fp16.onnx",
        approx_bytes: 229_804,
        sha256: "43241335a89e96cf3e933d31f0af41f2171e4f0ccba62a7d870fa9a5d7c77cc9",
    },
    ModelFileSpec {
        filename: "prompt_encoder_mask_decoder_fp16.onnx_data",
        url: "https://huggingface.co/onnx-community/sam3-tracker-ONNX/resolve/429305c8a5b3de597243d919a07e4e6bdcd00ef7/onnx/prompt_encoder_mask_decoder_fp16.onnx_data",
        approx_bytes: 11_011_072,
        sha256: "3c89f8f56c140806d65c37ec7b25ac70407d52cbfe6d8d0accbc5fff7ead562d",
    },
];

/// Static catalog of all downloadable segmentation model manifests.
pub const MODEL_SPECS: [ModelSpec; 3] = [
    ModelSpec {
        id: ModelId::Sam21HieraTinyFp16,
        slug: "sam2.1-hiera-tiny",
        display_name: "SAM 2.1 Hiera Tiny (FP16)",
        aliases: &["sam2.1", "sam2", "sam2.1-tiny"],
        engine: SegmentationEngine::Sam21Tiny,
        hf_repo: "onnx-community/sam2.1-hiera-tiny-ONNX",
        license: "Apache-2.0",
        license_url: "https://github.com/facebookresearch/sam2/blob/main/LICENSE",
        requires_license_acceptance: false,
        files: &SAM21_HIERA_TINY_FILES,
        input_size: 1024,
        norm_mean: [0.485, 0.456, 0.406],
        norm_std: [0.229, 0.224, 0.225],
        mask_size: 256,
        encoder_file: "vision_encoder_fp16.onnx",
        decoder_file: "prompt_encoder_mask_decoder_fp16.onnx",
        approx_bytes: 78_004_244,
    },
    ModelSpec {
        id: ModelId::Sam3TrackerQ4f16,
        slug: "sam3-tracker-q4f16",
        display_name: "SAM 3 Tracker (Q4F16)",
        aliases: &["sam3", "sam3-q4f16", "sam3-tracker"],
        engine: SegmentationEngine::Sam3Tracker,
        hf_repo: "onnx-community/sam3-tracker-ONNX",
        license: "Meta SAM License",
        license_url: "https://github.com/facebookresearch/sam3/blob/main/LICENSE",
        requires_license_acceptance: true,
        files: &SAM3_TRACKER_Q4F16_FILES,
        input_size: 1008,
        norm_mean: [0.5, 0.5, 0.5],
        norm_std: [0.5, 0.5, 0.5],
        mask_size: 288,
        encoder_file: "vision_encoder_q4f16.onnx",
        decoder_file: "prompt_encoder_mask_decoder_q4f16.onnx",
        approx_bytes: 302_295_094,
    },
    ModelSpec {
        id: ModelId::Sam3TrackerFp16,
        slug: "sam3-tracker-fp16",
        display_name: "SAM 3 Tracker (FP16)",
        aliases: &["sam3-fp16"],
        engine: SegmentationEngine::Sam3Tracker,
        hf_repo: "onnx-community/sam3-tracker-ONNX",
        license: "Meta SAM License",
        license_url: "https://github.com/facebookresearch/sam3/blob/main/LICENSE",
        requires_license_acceptance: true,
        files: &SAM3_TRACKER_FP16_FILES,
        input_size: 1008,
        norm_mean: [0.5, 0.5, 0.5],
        norm_std: [0.5, 0.5, 0.5],
        mask_size: 288,
        encoder_file: "vision_encoder_fp16.onnx",
        decoder_file: "prompt_encoder_mask_decoder_fp16.onnx",
        approx_bytes: 947_233_609,
    },
];

/// Returns the static [`ModelSpec`] for `id`.
pub const fn model_spec(id: ModelId) -> &'static ModelSpec {
    match id {
        ModelId::Sam21HieraTinyFp16 => &MODEL_SPECS[0],
        ModelId::Sam3TrackerQ4f16 => &MODEL_SPECS[1],
        ModelId::Sam3TrackerFp16 => &MODEL_SPECS[2],
    }
}

/// On-disk installation state of a downloadable ONNX model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelInstallState {
    NotInstalled,
    Partial { bytes_present: u64 },
    Installed { total_bytes: u64 },
}

impl ModelInstallState {
    pub const fn is_installed(self) -> bool {
        matches!(self, Self::Installed { .. })
    }

    pub const fn is_partial(self) -> bool {
        matches!(self, Self::Partial { .. })
    }

    pub const fn bytes_present(self) -> u64 {
        match self {
            Self::NotInstalled => 0,
            Self::Partial { bytes_present } => bytes_present,
            Self::Installed { total_bytes } => total_bytes,
        }
    }
}

/// Serializable status summary of a single downloadable model for GUI, CLI, and MCP.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelStatus {
    pub id: ModelId,
    pub display_name: String,
    pub engine: SegmentationEngine,
    pub hf_repo: String,
    pub license: String,
    pub license_url: String,
    pub requires_license_acceptance: bool,
    pub license_accepted: bool,
    pub approx_bytes: u64,
    pub install_dir: PathBuf,
    pub state: ModelInstallState,
}

/// Interactive point prompt in normalized `[0.0, 1.0]` image UV coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromptPoint {
    pub x: f32,
    pub y: f32,
    #[serde(default = "default_true")]
    pub positive: bool,
}

impl PromptPoint {
    pub const fn new(x: f32, y: f32, positive: bool) -> Self {
        Self { x, y, positive }
    }

    /// Clamps `(x, y)` to `[0.0, 1.0]`, replacing non-finite values with `0.5`.
    pub fn sanitized(self) -> Self {
        let c = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.5
            }
        };
        Self {
            x: c(self.x),
            y: c(self.y),
            positive: self.positive,
        }
    }
}

/// Interactive bounding box prompt in normalized `[0.0, 1.0]` image UV coordinates (`x0 <= x1, y0 <= y1`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromptBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl PromptBox {
    pub const fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    /// Clamps coordinates to `[0.0, 1.0]` and normalizes ordering so `x0 <= x1` and `y0 <= y1`.
    pub fn sanitized(self) -> Self {
        let c = |v: f32, def: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                def
            }
        };
        let a = c(self.x0, 0.0);
        let b = c(self.x1, 1.0);
        let c0 = c(self.y0, 0.0);
        let d = c(self.y1, 1.0);
        Self {
            x0: a.min(b),
            y0: c0.min(d),
            x1: a.max(b),
            y1: c0.max(d),
        }
    }

    /// Returns `true` when the box encloses a non-degenerate area.
    pub fn is_valid(&self) -> bool {
        self.x0.is_finite()
            && self.y0.is_finite()
            && self.x1.is_finite()
            && self.y1.is_finite()
            && self.x1 > self.x0
            && self.y1 > self.y0
    }
}

/// High-level segmentation prompt shared across GUI Smart Select, `rd` CLI, and MCP.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SmartMaskPrompt {
    /// Automatic foreground subject mask.
    AutoSubject,
    /// Automatic background mask (complement of foreground subject).
    AutoBackground,
    /// Interactive point and/or bounding-box prompted segmentation.
    Interactive {
        #[serde(default)]
        points: Vec<PromptPoint>,
        #[serde(default)]
        bbox: Option<PromptBox>,
        #[serde(default)]
        invert: bool,
    },
}

impl SmartMaskPrompt {
    /// Returns a sanitized copy with clamped coordinates and bounded point count.
    pub fn sanitized(&self) -> Self {
        match self {
            Self::AutoSubject => Self::AutoSubject,
            Self::AutoBackground => Self::AutoBackground,
            Self::Interactive {
                points,
                bbox,
                invert,
            } => Self::Interactive {
                points: points
                    .iter()
                    .take(MAX_PROMPT_POINTS)
                    .map(|p| p.sanitized())
                    .collect(),
                bbox: bbox.map(PromptBox::sanitized).filter(PromptBox::is_valid),
                invert: *invert,
            },
        }
    }
}

/// Persistent user configuration for AI segmentation engines and accepted third-party licenses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiModelConfig {
    pub active_engine: SegmentationEngine,
    pub accepted_licenses: Vec<String>,
    #[serde(default = "default_preferred_sam3_variant")]
    pub preferred_sam3_variant: ModelId,
}

impl Default for AiModelConfig {
    fn default() -> Self {
        Self {
            active_engine: SegmentationEngine::Auto,
            accepted_licenses: Vec::new(),
            preferred_sam3_variant: ModelId::Sam3TrackerQ4f16,
        }
    }
}

impl AiModelConfig {
    /// Returns `true` when `id` either needs no explicit license acceptance or its license has been accepted.
    pub fn is_license_accepted(&self, id: ModelId) -> bool {
        let spec = id.spec();
        if !spec.requires_license_acceptance {
            return true;
        }
        self.accepted_licenses.iter().any(|entry| {
            let trimmed = entry.trim();
            trimmed.eq_ignore_ascii_case(spec.license) || trimmed.eq_ignore_ascii_case(id.id())
        })
    }

    /// Records acceptance of `id`'s license if acceptance is required (deduplicated).
    pub fn accept_license(&mut self, id: ModelId) {
        let spec = id.spec();
        if spec.requires_license_acceptance {
            self.accept_license_name(spec.license);
        }
    }

    /// Records acceptance of a named license string (trimmed, deduplicated).
    pub fn accept_license_name(&mut self, license: &str) {
        let trimmed = license.trim();
        if trimmed.is_empty() {
            return;
        }
        if !self
            .accepted_licenses
            .iter()
            .any(|existing| existing.trim().eq_ignore_ascii_case(trimmed))
        {
            self.accepted_licenses.push(trimmed.to_owned());
        }
    }

    /// Returns a sanitized copy guaranteeing `preferred_sam3_variant` is a SAM 3 variant and
    /// `accepted_licenses` contains trimmed, deduplicated entries.
    pub fn sanitized(&self) -> Self {
        let preferred_sam3_variant = if self.preferred_sam3_variant.is_sam3() {
            self.preferred_sam3_variant
        } else {
            ModelId::Sam3TrackerQ4f16
        };
        let mut accepted_licenses = Vec::with_capacity(self.accepted_licenses.len());
        for entry in &self.accepted_licenses {
            let trimmed = entry.trim();
            if !trimmed.is_empty()
                && !accepted_licenses
                    .iter()
                    .any(|e: &String| e.eq_ignore_ascii_case(trimmed))
            {
                accepted_licenses.push(trimmed.to_owned());
            }
        }
        Self {
            active_engine: self.active_engine,
            accepted_licenses,
            preferred_sam3_variant,
        }
    }
}

/// Returns the root directory storing downloaded AI models.
///
/// Honors `RUNE_MODELS_DIR` if set and non-empty; otherwise falls back to
/// `$XDG_CONFIG_HOME/rune-darkroom/models` or `~/.config/rune-darkroom/models`.
pub fn models_root_dir() -> PathBuf {
    if let Some(override_dir) = std::env::var_os(RUNE_MODELS_DIR_ENV)
        && !override_dir.is_empty()
    {
        return PathBuf::from(override_dir);
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("rune-darkroom").join("models");
    }
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home)
            .join(".config")
            .join("rune-darkroom")
            .join("models");
    }
    PathBuf::from(".rune-darkroom-models")
}

/// Returns the on-disk directory for `id` under [`models_root_dir`].
pub fn model_dir(id: ModelId) -> PathBuf {
    model_dir_in(&models_root_dir(), id)
}

/// Returns the on-disk directory for `id` under an explicit `root` directory.
pub fn model_dir_in(root: &Path, id: ModelId) -> PathBuf {
    root.join(id.id())
}

/// Returns the path to `id`'s ONNX vision encoder file inside [`model_dir`].
pub fn model_encoder_path(id: ModelId) -> PathBuf {
    model_dir(id).join(id.spec().encoder_file)
}

/// Returns the path to `id`'s ONNX prompt encoder + mask decoder file inside [`model_dir`].
pub fn model_decoder_path(id: ModelId) -> PathBuf {
    model_dir(id).join(id.spec().decoder_file)
}

/// Inspects whether `id`'s required ONNX files are absent, partially downloaded, or installed
/// under [`models_root_dir`].
pub fn inspect_model_install(id: ModelId) -> ModelInstallState {
    inspect_model_install_in(&models_root_dir(), id)
}

/// Inspects whether `id`'s required ONNX files are absent, partially downloaded, or installed
/// under an explicit `root` directory.
pub fn inspect_model_install_in(root: &Path, id: ModelId) -> ModelInstallState {
    let dir = model_dir_in(root, id);
    let spec = id.spec();
    let mut non_empty_files = 0usize;
    let mut any_file_exists = false;
    let mut total_bytes = 0u64;

    for file in spec.files {
        let path = dir.join(file.filename);
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.is_file()
        {
            any_file_exists = true;
            let len = meta.len();
            total_bytes = total_bytes.saturating_add(len);
            if len > 0 {
                non_empty_files += 1;
            }
        } else {
            let part_path = dir.join(format!("{}.part", file.filename));
            if let Ok(meta) = std::fs::metadata(&part_path)
                && meta.is_file()
                && meta.len() > 0
            {
                any_file_exists = true;
                total_bytes = total_bytes.saturating_add(meta.len());
            }
        }
    }

    if non_empty_files == spec.files.len() {
        ModelInstallState::Installed { total_bytes }
    } else if any_file_exists {
        ModelInstallState::Partial {
            bytes_present: total_bytes,
        }
    } else {
        ModelInstallState::NotInstalled
    }
}

/// Builds a serializable [`ModelStatus`] for `id` against [`models_root_dir`] and `config`.
pub fn inspect_model_status(id: ModelId, config: &AiModelConfig) -> ModelStatus {
    inspect_model_status_in(&models_root_dir(), id, config)
}

/// Builds a serializable [`ModelStatus`] for `id` against an explicit `root` directory and `config`.
pub fn inspect_model_status_in(root: &Path, id: ModelId, config: &AiModelConfig) -> ModelStatus {
    let spec = id.spec();
    ModelStatus {
        id,
        display_name: spec.display_name.to_owned(),
        engine: spec.engine,
        hf_repo: spec.hf_repo.to_owned(),
        license: spec.license.to_owned(),
        license_url: spec.license_url.to_owned(),
        requires_license_acceptance: spec.requires_license_acceptance,
        license_accepted: config.is_license_accepted(id),
        approx_bytes: spec.approx_bytes,
        install_dir: model_dir_in(root, id),
        state: inspect_model_install_in(root, id),
    }
}

/// Builds [`ModelStatus`] summaries for all downloadable models under [`models_root_dir`].
pub fn inspect_all_model_statuses(config: &AiModelConfig) -> Vec<ModelStatus> {
    inspect_all_model_statuses_in(&models_root_dir(), config)
}

/// Builds [`ModelStatus`] summaries for all downloadable models under an explicit `root` directory.
pub fn inspect_all_model_statuses_in(root: &Path, config: &AiModelConfig) -> Vec<ModelStatus> {
    ModelId::ALL
        .iter()
        .map(|&id| inspect_model_status_in(root, id, config))
        .collect()
}

/// Returns the path to `ai_models.json`.
///
/// When `RUNE_MODELS_DIR` is set (for hermetic tests or portable installs):
/// - If it ends with `"models"` and has a non-empty parent, returns `<parent>/ai_models.json`.
/// - Otherwise returns `<RUNE_MODELS_DIR>/ai_models.json` so parallel temp dirs stay isolated.
///
/// Otherwise returns `$XDG_CONFIG_HOME/rune-darkroom/ai_models.json` or
/// `~/.config/rune-darkroom/ai_models.json`.
pub fn ai_model_config_path() -> PathBuf {
    if let Some(override_dir) = std::env::var_os(RUNE_MODELS_DIR_ENV)
        && !override_dir.is_empty()
    {
        let p = PathBuf::from(override_dir);
        if p.file_name().is_some_and(|n| n == "models")
            && let Some(parent) = p.parent()
            && !parent.as_os_str().is_empty()
        {
            return parent.join("ai_models.json");
        }
        return p.join("ai_models.json");
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg)
            .join("rune-darkroom")
            .join("ai_models.json");
    }
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home)
            .join(".config")
            .join("rune-darkroom")
            .join("ai_models.json");
    }
    PathBuf::from(".rune-darkroom-ai-models.json")
}

/// Loads [`AiModelConfig`] from [`ai_model_config_path`], returning [`AiModelConfig::default`]
/// if the config file does not exist yet.
pub fn load_ai_model_config() -> Result<AiModelConfig, CoreError> {
    load_ai_model_config_from(&ai_model_config_path())
}

/// Loads [`AiModelConfig`] from an explicit file `path`, returning [`AiModelConfig::default`]
/// if `path` does not exist.
pub fn load_ai_model_config_from(path: &Path) -> Result<AiModelConfig, CoreError> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let parsed: AiModelConfig = serde_json::from_slice(&bytes)?;
            Ok(parsed.sanitized())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(AiModelConfig::default()),
        Err(err) => Err(CoreError::Io(err)),
    }
}

/// Atomically persists `config` to [`ai_model_config_path`] via temp-dotfile + `sync_all` + `rename`.
pub fn save_ai_model_config(config: &AiModelConfig) -> Result<PathBuf, CoreError> {
    let path = ai_model_config_path();
    save_ai_model_config_to(&path, config)?;
    Ok(path)
}

/// Atomically persists `config` to an explicit `path` via temp-dotfile + `sync_all` + `rename`.
pub fn save_ai_model_config_to(path: &Path, config: &AiModelConfig) -> Result<(), CoreError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let clean = config.sanitized();
    let json = serde_json::to_string_pretty(&clean)?;
    atomic_write_bytes(path, json.as_bytes())
}

/// Performs a locked read-modify-write update on [`ai_model_config_path`], preventing concurrent
/// GUI/CLI/MCP writers from clobbering each other and refusing to overwrite a corrupt JSON file.
pub fn update_ai_model_config(
    mutate: impl FnOnce(&mut AiModelConfig),
) -> Result<AiModelConfig, CoreError> {
    let path = ai_model_config_path();
    update_ai_model_config_at(&path, mutate)
}

/// Performs a locked read-modify-write update on an explicit `ai_models.json` `path`,
/// returning an error without modifying disk if the existing file fails to parse.
pub fn update_ai_model_config_at(
    path: &Path,
    mutate: impl FnOnce(&mut AiModelConfig),
) -> Result<AiModelConfig, CoreError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = acquire_path_lock(path)?;
    let mut config = load_ai_model_config_from(path)?;
    mutate(&mut config);
    let clean = config.sanitized();
    save_ai_model_config_to(path, &clean)?;
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "rune_core_models_{tag}_{}_{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn segmentation_engine_parsing_and_serde() {
        assert_eq!(SegmentationEngine::default(), SegmentationEngine::Auto);
        for engine in SegmentationEngine::ALL {
            assert_eq!(
                SegmentationEngine::from_str_loose(engine.as_str()),
                Some(engine)
            );
            assert!(!engine.display_name().is_empty());
            let json = serde_json::to_string(&engine).unwrap();
            let back: SegmentationEngine = serde_json::from_str(&json).unwrap();
            assert_eq!(back, engine);
        }
        assert_eq!(
            SegmentationEngine::from_str_loose("apple-vision"),
            Some(SegmentationEngine::AppleVision)
        );
        assert_eq!(
            SegmentationEngine::from_str_loose("SAM2.1"),
            Some(SegmentationEngine::Sam21Tiny)
        );
        assert_eq!(
            SegmentationEngine::from_str_loose("sam3"),
            Some(SegmentationEngine::Sam3Tracker)
        );
        assert_eq!(
            SegmentationEngine::from_str_loose("fallback"),
            Some(SegmentationEngine::Heuristic)
        );
        assert_eq!(SegmentationEngine::from_str_loose("unknown_backend"), None);
    }

    #[test]
    fn model_specs_and_aliases_are_consistent() {
        for &id in &ModelId::ALL {
            let spec = id.spec();
            assert_eq!(spec.id, id);
            assert_eq!(spec.slug, id.id());
            assert_eq!(spec.engine, id.engine());
            assert_eq!(ModelId::from_str_loose(id.id()), Some(id));
            for &alias in id.aliases() {
                assert_eq!(
                    ModelId::from_str_loose(alias),
                    Some(id),
                    "alias '{alias}' failed to resolve to {id:?}"
                );
            }

            // Byte sums and encoder/decoder filenames must match the files manifest exactly.
            let sum: u64 = spec.files.iter().map(|f| f.approx_bytes).sum();
            assert_eq!(sum, spec.approx_bytes, "approx_bytes mismatch for {id:?}");
            assert!(
                spec.files.iter().any(|f| f.filename == spec.encoder_file),
                "encoder_file missing in manifest for {id:?}"
            );
            assert!(
                spec.files.iter().any(|f| f.filename == spec.decoder_file),
                "decoder_file missing in manifest for {id:?}"
            );
            for f in spec.files {
                assert!(
                    f.url.starts_with("https://huggingface.co/"),
                    "unexpected URL for {id:?}: {}",
                    f.url
                );
                assert!(
                    !f.url.contains("/resolve/main/"),
                    "model URL must pin an immutable commit SHA, not mutable main: {}",
                    f.url
                );
                assert_eq!(
                    f.sha256.len(),
                    64,
                    "expected 64-char SHA-256 hex digest for {}",
                    f.filename
                );
                assert!(
                    f.sha256
                        .bytes()
                        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                    "invalid lowercase hex SHA-256 for {}",
                    f.filename
                );
                assert!(f.url.contains(spec.hf_repo));
                assert!(f.url.ends_with(f.filename));
            }

            // Serde uses canonical hyphenated ID.
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, format!("\"{}\"", id.id()));
            let back: ModelId = serde_json::from_str(&json).unwrap();
            assert_eq!(back, id);
        }

        // Exact preprocessing constants
        let sam21 = ModelId::Sam21HieraTinyFp16.spec();
        assert_eq!(sam21.input_size, 1024);
        assert_eq!(sam21.mask_size, 256);
        assert!(!sam21.requires_license_acceptance);
        assert_eq!(sam21.license, "Apache-2.0");

        let sam3_q4 = ModelId::Sam3TrackerQ4f16.spec();
        assert_eq!(sam3_q4.input_size, 1008);
        assert_eq!(sam3_q4.mask_size, 288);
        assert!(sam3_q4.requires_license_acceptance);
        assert_eq!(sam3_q4.license, "Meta SAM License");

        let sam3_fp16 = ModelId::Sam3TrackerFp16.spec();
        assert_eq!(sam3_fp16.input_size, 1008);
        assert_eq!(sam3_fp16.mask_size, 288);
        assert!(sam3_fp16.requires_license_acceptance);
    }

    #[test]
    fn install_state_inspection_tracks_missing_partial_and_installed() {
        let root = unique_temp_dir("install");
        let id = ModelId::Sam21HieraTinyFp16;
        let dir = model_dir_in(&root, id);

        assert_eq!(
            inspect_model_install_in(&root, id),
            ModelInstallState::NotInstalled
        );

        std::fs::create_dir_all(&dir).unwrap();
        // Write only first file -> Partial
        let first = &id.spec().files[0];
        std::fs::write(dir.join(first.filename), vec![1u8; 128]).unwrap();
        assert_eq!(
            inspect_model_install_in(&root, id),
            ModelInstallState::Partial { bytes_present: 128 }
        );

        // Write remaining files with non-zero lengths -> Installed
        let mut expected_total = 128u64;
        for (idx, file) in id.spec().files.iter().enumerate().skip(1) {
            let sz = (idx + 1) * 64;
            std::fs::write(dir.join(file.filename), vec![2u8; sz]).unwrap();
            expected_total += sz as u64;
        }
        assert_eq!(
            inspect_model_install_in(&root, id),
            ModelInstallState::Installed {
                total_bytes: expected_total
            }
        );

        // Truncating one file to 0 bytes degrades back to Partial
        let last = id.spec().files.last().unwrap();
        std::fs::write(dir.join(last.filename), b"").unwrap();
        assert!(inspect_model_install_in(&root, id).is_partial());

        // A `.part`-only download must report Partial (not NotInstalled) so GUI can offer Resume.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.part", first.filename)),
            vec![9u8; 4096],
        )
        .unwrap();
        assert_eq!(
            inspect_model_install_in(&root, id),
            ModelInstallState::Partial {
                bytes_present: 4096
            }
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ai_model_config_atomic_roundtrip_and_license_tracking() {
        let root = unique_temp_dir("config");
        let cfg_path = root.join("ai_models.json");

        // Missing file loads default cleanly
        let initial = load_ai_model_config_from(&cfg_path).unwrap();
        assert_eq!(initial, AiModelConfig::default());
        assert!(initial.is_license_accepted(ModelId::Sam21HieraTinyFp16));
        assert!(!initial.is_license_accepted(ModelId::Sam3TrackerQ4f16));
        assert!(!initial.is_license_accepted(ModelId::Sam3TrackerFp16));

        let mut updated = AiModelConfig {
            active_engine: SegmentationEngine::Sam3Tracker,
            accepted_licenses: vec![],
            preferred_sam3_variant: ModelId::Sam3TrackerFp16,
        };
        updated.accept_license(ModelId::Sam3TrackerQ4f16);
        updated.accept_license(ModelId::Sam3TrackerFp16); // deduplicated
        assert_eq!(updated.accepted_licenses, vec!["Meta SAM License"]);
        assert!(updated.is_license_accepted(ModelId::Sam3TrackerQ4f16));
        assert!(updated.is_license_accepted(ModelId::Sam3TrackerFp16));

        save_ai_model_config_to(&cfg_path, &updated).unwrap();
        let reloaded = load_ai_model_config_from(&cfg_path).unwrap();
        assert_eq!(reloaded, updated);

        // ModelStatus summaries reflect license acceptance & install dir
        let statuses = inspect_all_model_statuses_in(&root, &reloaded);
        assert_eq!(statuses.len(), 3);
        assert!(statuses.iter().all(|s| s.license_accepted));

        // Non-SAM3 preferred_sam3_variant gets sanitized back to Sam3TrackerQ4f16
        let bad = AiModelConfig {
            active_engine: SegmentationEngine::Auto,
            accepted_licenses: vec![
                "  Meta SAM License  ".to_owned(),
                "meta sam license".to_owned(),
            ],
            preferred_sam3_variant: ModelId::Sam21HieraTinyFp16,
        };
        let clean = bad.sanitized();
        assert_eq!(clean.preferred_sam3_variant, ModelId::Sam3TrackerQ4f16);
        assert_eq!(clean.accepted_licenses, vec!["Meta SAM License"]);

        // update_ai_model_config_at preserves existing fields and refuses to wipe corrupt JSON.
        let merged = update_ai_model_config_at(&cfg_path, |cfg| {
            cfg.active_engine = SegmentationEngine::AppleVision;
        })
        .unwrap();
        assert_eq!(merged.active_engine, SegmentationEngine::AppleVision);
        assert!(merged.is_license_accepted(ModelId::Sam3TrackerQ4f16));

        std::fs::write(&cfg_path, b"{not valid json").unwrap();
        assert!(
            update_ai_model_config_at(&cfg_path, |cfg| {
                cfg.active_engine = SegmentationEngine::Heuristic;
            })
            .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            "{not valid json"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prompt_types_sanitize_and_roundtrip_serde() {
        let prompt = SmartMaskPrompt::Interactive {
            points: vec![
                PromptPoint::new(-0.2, 1.5, true),
                PromptPoint::new(f32::NAN, 0.4, false),
            ],
            bbox: Some(PromptBox::new(0.8, 0.9, 0.2, 0.1)),
            invert: true,
        };
        let clean = prompt.sanitized();
        assert_eq!(
            clean,
            SmartMaskPrompt::Interactive {
                points: vec![
                    PromptPoint::new(0.0, 1.0, true),
                    PromptPoint::new(0.5, 0.4, false),
                ],
                bbox: Some(PromptBox::new(0.2, 0.1, 0.8, 0.9)),
                invert: true,
            }
        );

        let json = serde_json::to_string(&clean).unwrap();
        let back: SmartMaskPrompt = serde_json::from_str(&json).unwrap();
        assert_eq!(back, clean);

        // Degenerate zero-area box is dropped by sanitization
        let degen = SmartMaskPrompt::Interactive {
            points: vec![],
            bbox: Some(PromptBox::new(0.5, 0.5, 0.5, 0.5)),
            invert: false,
        };
        assert_eq!(
            degen.sanitized(),
            SmartMaskPrompt::Interactive {
                points: vec![],
                bbox: None,
                invert: false,
            }
        );
    }
}
