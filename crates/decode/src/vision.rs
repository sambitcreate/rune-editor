//! Unified AI Subject, Background & Interactive Smart Mask segmentation (`512×512` R8).
//!
//! Routes prompts seamlessly across:
//! 1. **Apple Vision ANE** (`macOS 14+` `VNGenerateForegroundInstanceMaskRequest`, `macOS 12–13`
//!    `VNGeneratePersonSegmentationRequest`) — built-in, zero download.
//! 2. **ONNX Runtime SAM 3 Tracker / SAM 2.1 Hiera Tiny** (`CoreML EP` + CPU fallback) — optional
//!    download-on-command interactive point & bounding-box segmentation with cached image embeddings.
//! 3. **Perceptual CIE Lab Heuristic** — deterministic cross-platform fallback supporting both
//!    automatic foreground saliency and interactive point/box GrabCut-style segmentation.

use std::time::Instant;

use rune_editor_core::{
    AiModelConfig, LAYER_MASK_TILE_SIZE, MaskShape, ModelId, PromptBox, PromptPoint,
    SegmentationEngine, SmartMaskPrompt, inspect_model_install, load_ai_model_config,
    rgb_to_hsl_and_lab,
};
use thiserror::Error;

use crate::LinearImage;

/// Errors returned by [`generate_smart_mask`].
#[derive(Debug, Error)]
pub enum VisionError {
    #[error("Cannot generate mask for an empty image")]
    EmptyImage,
    #[error("Segmentation backend unavailable: {0}")]
    BackendUnavailable(String),
    #[error(
        "Segmentation model '{0}' is not installed. Download or configure it first via model setup."
    )]
    ModelNotInstalled(ModelId),
    #[error("Segmentation inference failed: {0}")]
    InferenceFailed(String),
}

/// Structured output from [`generate_smart_mask`].
#[derive(Clone, Debug)]
pub struct SmartMaskResult {
    pub mask: MaskShape,
    pub engine_used: SegmentationEngine,
    pub backend_label: String,
    pub elapsed_ms: u64,
    pub instance_count: Option<usize>,
    pub iou_score: Option<f32>,
    pub fallback_note: Option<String>,
}

/// Backwards-compatible subject/background mask entry point (`512×512` R8 `MaskShape::RasterMask`).
///
/// Routes through [`generate_smart_mask`] with [`SmartMaskPrompt::AutoSubject`] or
/// [`SmartMaskPrompt::AutoBackground`] using the configured engine and graceful fallback.
pub fn generate_subject_mask(image: &LinearImage, background: bool) -> MaskShape {
    let prompt = if background {
        SmartMaskPrompt::AutoBackground
    } else {
        SmartMaskPrompt::AutoSubject
    };
    match generate_smart_mask(image, &prompt, None) {
        Ok(res) => res.mask,
        Err(_) => generate_heuristic_mask(image, background),
    }
}

/// Generates a crisp `512×512` R8 `MaskShape::RasterMask` for automatic subject/background or
/// interactive point/box prompts using the active or overridden [`SegmentationEngine`].
///
/// Resolution order for [`SegmentationEngine::Auto`]:
/// - **AutoSubject / AutoBackground:** Apple Vision ANE (macOS) → installed SAM 3 / SAM 2.1 → Lab heuristic.
/// - **Interactive (points/boxes):** installed SAM 3 → installed SAM 2.1 → Apple Vision ANE (macOS) → local interactive Lab fallback.
pub fn generate_smart_mask(
    image: &LinearImage,
    prompt: &SmartMaskPrompt,
    engine_override: Option<SegmentationEngine>,
) -> Result<SmartMaskResult, VisionError> {
    if !image.has_valid_buffer() {
        return Err(VisionError::EmptyImage);
    }

    let started = Instant::now();
    let prompt = prompt.sanitized();
    let config = load_ai_model_config().unwrap_or_default();
    let requested = engine_override.unwrap_or(config.active_engine);
    let explicit_override = engine_override.is_some();

    match requested {
        SegmentationEngine::Auto => run_auto_chain(image, &prompt, &config, None, None, started),
        SegmentationEngine::Heuristic => Ok(run_heuristic_result(image, &prompt, None, started)),
        SegmentationEngine::AppleVision => match run_apple_vision(image, &prompt, started) {
            Ok(res) => Ok(res),
            Err(err) if explicit_override => Err(err),
            Err(err) => run_auto_chain(
                image,
                &prompt,
                &config,
                Some(SegmentationEngine::AppleVision),
                Some(format!("Apple Vision unavailable ({err}); fell back")),
                started,
            ),
        },
        SegmentationEngine::Sam21Tiny => {
            match run_sam_model(image, &prompt, ModelId::Sam21HieraTinyFp16, started) {
                Ok(res) => Ok(res),
                Err(err) if explicit_override => Err(err),
                Err(err) => run_auto_chain(
                    image,
                    &prompt,
                    &config,
                    Some(SegmentationEngine::Sam21Tiny),
                    Some(format!("SAM 2.1 unavailable ({err}); fell back")),
                    started,
                ),
            }
        }
        SegmentationEngine::Sam3Tracker => {
            let model_id = resolve_sam3_model_id(&config);
            match run_sam_model(image, &prompt, model_id, started) {
                Ok(res) => Ok(res),
                Err(err) if explicit_override => Err(err),
                Err(err) => run_auto_chain(
                    image,
                    &prompt,
                    &config,
                    Some(SegmentationEngine::Sam3Tracker),
                    Some(format!("SAM 3 unavailable ({err}); fell back")),
                    started,
                ),
            }
        }
    }
}

fn resolve_sam3_model_id(config: &AiModelConfig) -> ModelId {
    let preferred = if config.preferred_sam3_variant.is_sam3() {
        config.preferred_sam3_variant
    } else {
        ModelId::Sam3TrackerQ4f16
    };
    if inspect_model_install(preferred).is_installed() {
        return preferred;
    }
    let alternate = if preferred == ModelId::Sam3TrackerQ4f16 {
        ModelId::Sam3TrackerFp16
    } else {
        ModelId::Sam3TrackerQ4f16
    };
    if inspect_model_install(alternate).is_installed() {
        return alternate;
    }
    preferred
}

fn run_auto_chain(
    image: &LinearImage,
    prompt: &SmartMaskPrompt,
    config: &AiModelConfig,
    skip_engine: Option<SegmentationEngine>,
    initial_note: Option<String>,
    started: Instant,
) -> Result<SmartMaskResult, VisionError> {
    let mut notes: Vec<String> = initial_note.into_iter().collect();
    let is_interactive = matches!(prompt, SmartMaskPrompt::Interactive { .. });

    if !is_interactive {
        // AutoSubject / AutoBackground order: Apple Vision ANE -> installed SAM 3 -> SAM 2.1 -> Heuristic.
        #[cfg(target_os = "macos")]
        if skip_engine != Some(SegmentationEngine::AppleVision)
            && crate::vision_apple::is_available()
        {
            match run_apple_vision(image, prompt, started) {
                Ok(mut res) => {
                    res.fallback_note = join_notes(&notes);
                    return Ok(res);
                }
                Err(err) => notes.push(format!("Apple Vision: {err}")),
            }
        }

        if skip_engine != Some(SegmentationEngine::Sam3Tracker) {
            let sam3_id = resolve_sam3_model_id(config);
            if inspect_model_install(sam3_id).is_installed() {
                match run_sam_model(image, prompt, sam3_id, started) {
                    Ok(mut res) => {
                        res.fallback_note = join_notes(&notes);
                        return Ok(res);
                    }
                    Err(err) => notes.push(format!("SAM 3: {err}")),
                }
            }
        }

        if skip_engine != Some(SegmentationEngine::Sam21Tiny)
            && inspect_model_install(ModelId::Sam21HieraTinyFp16).is_installed()
        {
            match run_sam_model(image, prompt, ModelId::Sam21HieraTinyFp16, started) {
                Ok(mut res) => {
                    res.fallback_note = join_notes(&notes);
                    return Ok(res);
                }
                Err(err) => notes.push(format!("SAM 2.1: {err}")),
            }
        }

        return Ok(run_heuristic_result(
            image,
            prompt,
            join_notes(&notes),
            started,
        ));
    }

    // Interactive order: installed SAM 3 -> installed SAM 2.1 -> Apple Vision ANE -> Interactive Lab fallback.
    if skip_engine != Some(SegmentationEngine::Sam3Tracker) {
        let sam3_id = resolve_sam3_model_id(config);
        if inspect_model_install(sam3_id).is_installed() {
            match run_sam_model(image, prompt, sam3_id, started) {
                Ok(mut res) => {
                    res.fallback_note = join_notes(&notes);
                    return Ok(res);
                }
                Err(err) => notes.push(format!("SAM 3: {err}")),
            }
        }
    }

    if skip_engine != Some(SegmentationEngine::Sam21Tiny)
        && inspect_model_install(ModelId::Sam21HieraTinyFp16).is_installed()
    {
        match run_sam_model(image, prompt, ModelId::Sam21HieraTinyFp16, started) {
            Ok(mut res) => {
                res.fallback_note = join_notes(&notes);
                return Ok(res);
            }
            Err(err) => notes.push(format!("SAM 2.1: {err}")),
        }
    }

    #[cfg(target_os = "macos")]
    if skip_engine != Some(SegmentationEngine::AppleVision) && crate::vision_apple::is_available() {
        match run_apple_vision(image, prompt, started) {
            Ok(mut res) => {
                res.fallback_note = join_notes(&notes);
                return Ok(res);
            }
            Err(err) => notes.push(format!("Apple Vision: {err}")),
        }
    }

    Ok(run_heuristic_result(
        image,
        prompt,
        join_notes(&notes),
        started,
    ))
}

fn join_notes(notes: &[String]) -> Option<String> {
    if notes.is_empty() {
        None
    } else {
        Some(notes.join("; "))
    }
}

fn run_apple_vision(
    image: &LinearImage,
    prompt: &SmartMaskPrompt,
    started: Instant,
) -> Result<SmartMaskResult, VisionError> {
    #[cfg(target_os = "macos")]
    {
        if !crate::vision_apple::is_available() {
            return Err(VisionError::BackendUnavailable(
                "Apple Vision foreground/person segmentation requires macOS 12+".to_owned(),
            ));
        }
        let res =
            crate::vision_apple::segment(image, prompt).map_err(VisionError::InferenceFailed)?;
        Ok(SmartMaskResult {
            mask: res.mask,
            engine_used: SegmentationEngine::AppleVision,
            backend_label: res.backend_label.to_owned(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            instance_count: Some(res.instance_count),
            iou_score: None,
            fallback_note: None,
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (image, prompt, started);
        Err(VisionError::BackendUnavailable(
            "Apple Vision is only available on macOS".to_owned(),
        ))
    }
}

fn run_sam_model(
    image: &LinearImage,
    prompt: &SmartMaskPrompt,
    model_id: ModelId,
    started: Instant,
) -> Result<SmartMaskResult, VisionError> {
    if !inspect_model_install(model_id).is_installed() {
        return Err(VisionError::ModelNotInstalled(model_id));
    }
    let res =
        crate::sam_ort::segment(image, prompt, model_id).map_err(VisionError::InferenceFailed)?;
    Ok(SmartMaskResult {
        mask: res.mask,
        engine_used: model_id.engine(),
        backend_label: res.backend_label.to_owned(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        instance_count: None,
        iou_score: Some(res.iou_score),
        fallback_note: None,
    })
}

fn run_heuristic_result(
    image: &LinearImage,
    prompt: &SmartMaskPrompt,
    fallback_note: Option<String>,
    started: Instant,
) -> SmartMaskResult {
    let (mask, label) = match prompt {
        SmartMaskPrompt::AutoSubject => (
            generate_heuristic_mask(image, false),
            "Perceptual CIE Lab Saliency (CPU Heuristic)",
        ),
        SmartMaskPrompt::AutoBackground => (
            generate_heuristic_mask(image, true),
            "Perceptual CIE Lab Saliency (CPU Heuristic)",
        ),
        SmartMaskPrompt::Interactive {
            points,
            bbox,
            invert,
        } => {
            if points.is_empty() && bbox.is_none() {
                (
                    generate_heuristic_mask(image, *invert),
                    "Perceptual CIE Lab Saliency (CPU Heuristic)",
                )
            } else {
                (
                    generate_interactive_heuristic_mask(image, points, bbox.as_ref(), *invert),
                    "Interactive CIE Lab GrabCut (CPU Heuristic)",
                )
            }
        }
    };

    SmartMaskResult {
        mask,
        engine_used: SegmentationEngine::Heuristic,
        backend_label: label.to_owned(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        instance_count: None,
        iou_score: None,
        fallback_note,
    }
}

/// Offline, deterministic perceptual CIE Lab saliency heuristic (`512×512` R8 `MaskShape::RasterMask`).
///
/// Used directly when `SegmentationEngine::Heuristic` is requested, and as the cross-platform
/// fallback whenever neither Apple Vision nor an ONNX SAM model is available.
pub fn generate_heuristic_mask(image: &LinearImage, background: bool) -> MaskShape {
    let grid = LAYER_MASK_TILE_SIZE;
    let total = grid * grid;
    if !image.has_valid_buffer() {
        let fill = if background { 255u8 } else { 0u8 };
        return MaskShape::from_raster_u8(grid as u32, grid as u32, &vec![fill; total]);
    }

    // Evaluate saliency + joint-bilateral refinement on a crisp 256×256 Lab working grid,
    // then bilinearly upsample to `LAYER_MASK_TILE_SIZE × LAYER_MASK_TILE_SIZE` (512×512)
    // so fallback generation stays sub-10ms even on CPU.
    let work = 256usize;
    let lab_grid = downsample_linear_to_lab_grid(image, work);
    let mut prob = compute_auto_saliency_prob(&lab_grid, work);

    let mut scratch = vec![0.0_f32; work * work];
    for _ in 0..2 {
        joint_bilateral_pass(&prob, &lab_grid, work, &mut scratch);
        std::mem::swap(&mut prob, &mut scratch);
    }

    upsample_prob_to_raster(&prob, work, grid, background)
}

/// Local interactive CIE Lab GrabCut-style fallback for point and/or bounding-box prompts.
pub fn generate_interactive_heuristic_mask(
    image: &LinearImage,
    points: &[PromptPoint],
    bbox: Option<&PromptBox>,
    invert: bool,
) -> MaskShape {
    let grid = LAYER_MASK_TILE_SIZE;
    let total = grid * grid;
    if !image.has_valid_buffer() {
        let fill = if invert { 255u8 } else { 0u8 };
        return MaskShape::from_raster_u8(grid as u32, grid as u32, &vec![fill; total]);
    }

    let work = 256usize;
    let lab_grid = downsample_linear_to_lab_grid(image, work);

    let mut pos_protos: Vec<[f32; 3]> = Vec::new();
    let mut pos_coords: Vec<[f32; 2]> = Vec::new();
    let mut neg_protos: Vec<[f32; 3]> = Vec::new();
    let mut neg_coords: Vec<[f32; 2]> = Vec::new();

    for pt in points {
        let lab = sample_patch_lab(&lab_grid, work, pt.x, pt.y, 3);
        if pt.positive {
            pos_protos.push(lab);
            pos_coords.push([pt.x, pt.y]);
        } else {
            neg_protos.push(lab);
            neg_coords.push([pt.x, pt.y]);
        }
    }

    if let Some(b) = bbox {
        if pos_protos.is_empty() {
            // Sample 5 interior points of the bounding box as foreground prototypes.
            let cx = (b.x0 + b.x1) * 0.5;
            let cy = (b.y0 + b.y1) * 0.5;
            let dx = (b.x1 - b.x0) * 0.22;
            let dy = (b.y1 - b.y0) * 0.22;
            for [px, py] in [
                [cx, cy],
                [cx - dx, cy],
                [cx + dx, cy],
                [cx, cy - dy],
                [cx, cy + dy],
            ] {
                pos_protos.push(sample_patch_lab(&lab_grid, work, px, py, 3));
                pos_coords.push([px, py]);
            }
        }
        // Sample just outside the bounding box as background prototypes.
        let pad = 0.04;
        for [nx, ny] in [
            [(b.x0 - pad).clamp(0.0, 1.0), (b.y0 + b.y1) * 0.5],
            [(b.x1 + pad).clamp(0.0, 1.0), (b.y0 + b.y1) * 0.5],
            [(b.x0 + b.x1) * 0.5, (b.y0 - pad).clamp(0.0, 1.0)],
            [(b.x0 + b.x1) * 0.5, (b.y1 + pad).clamp(0.0, 1.0)],
        ] {
            neg_protos.push(sample_patch_lab(&lab_grid, work, nx, ny, 3));
        }
    }

    if pos_protos.is_empty() {
        return generate_heuristic_mask(image, invert);
    }

    if neg_protos.is_empty() {
        // Add 4 frame border prototypes when user provided only positive point(s).
        for [nx, ny] in [[0.03, 0.03], [0.97, 0.03], [0.03, 0.97], [0.97, 0.97]] {
            neg_protos.push(sample_patch_lab(&lab_grid, work, nx, ny, 4));
        }
    }

    let mut prob = vec![0.0_f32; work * work];
    for gy in 0..work {
        let v = (gy as f32 + 0.5) / work as f32;
        let row = gy * work;
        for gx in 0..work {
            let u = (gx as f32 + 0.5) / work as f32;
            let lab = lab_grid[row + gx];

            let d_pos = pos_protos
                .iter()
                .map(|&p| lab_dist(lab, p))
                .fold(f32::MAX, f32::min);
            let d_neg = neg_protos
                .iter()
                .map(|&p| lab_dist(lab, p))
                .fold(f32::MAX, f32::min);

            let color_score = d_neg / (d_pos + d_neg + 1e-3);

            let spatial_pos = pos_coords
                .iter()
                .map(|&[px, py]| {
                    let dx = u - px;
                    let dy = v - py;
                    (-3.5 * (dx * dx + dy * dy)).exp()
                })
                .fold(0.0_f32, f32::max);

            let spatial_neg = neg_coords
                .iter()
                .map(|&[nx, ny]| {
                    let dx = u - nx;
                    let dy = v - ny;
                    (-10.0 * (dx * dx + dy * dy)).exp()
                })
                .fold(0.0_f32, f32::max);

            let box_weight = if let Some(b) = bbox {
                let inside_x = if u < b.x0 {
                    b.x0 - u
                } else if u > b.x1 {
                    u - b.x1
                } else {
                    0.0
                };
                let inside_y = if v < b.y0 {
                    b.y0 - v
                } else if v > b.y1 {
                    v - b.y1
                } else {
                    0.0
                };
                let outside_dist = inside_x.hypot(inside_y);
                (-outside_dist * 24.0).exp()
            } else {
                1.0
            };

            let combined =
                (color_score * 0.72 + spatial_pos * 0.28) * (1.0 - spatial_neg * 0.75) * box_weight;
            prob[row + gx] = smoothstep(0.36, 0.66, combined);
        }
    }

    let mut scratch = vec![0.0_f32; work * work];
    for _ in 0..2 {
        joint_bilateral_pass(&prob, &lab_grid, work, &mut scratch);
        std::mem::swap(&mut prob, &mut scratch);
    }

    upsample_prob_to_raster(&prob, work, grid, invert)
}

/// Estimates the foreground saliency centroid `[u, v]` in `[0, 1]^2` for seeding SAM
/// automatic subject segmentation on platforms without Apple Vision.
pub(crate) fn estimate_saliency_centroid(image: &LinearImage) -> [f32; 2] {
    if !image.has_valid_buffer() {
        return [0.5, 0.5];
    }
    let work = 64usize;
    let lab_grid = downsample_linear_to_lab_grid(image, work);
    let prob = compute_auto_saliency_prob(&lab_grid, work);

    let mut sum_x = 0.0_f64;
    let mut sum_y = 0.0_f64;
    let mut w_sum = 0.0_f64;
    for y in 0..work {
        let v = (y as f64 + 0.5) / work as f64;
        let row = y * work;
        for x in 0..work {
            let u = (x as f64 + 0.5) / work as f64;
            let p = prob[row + x] as f64;
            if p > 0.45 {
                let w = p * p;
                sum_x += u * w;
                sum_y += v * w;
                w_sum += w;
            }
        }
    }
    if w_sum > 1e-6 {
        [
            (sum_x / w_sum).clamp(0.1, 0.9) as f32,
            (sum_y / w_sum).clamp(0.1, 0.9) as f32,
        ]
    } else {
        [0.5, 0.5]
    }
}

fn downsample_linear_to_lab_grid(image: &LinearImage, grid: usize) -> Vec<[f32; 3]> {
    let sw = image.width as usize;
    let sh = image.height as usize;
    let mut lab_grid = vec![[0.0_f32; 3]; grid * grid];
    for gy in 0..grid {
        let sy = ((gy as f32 + 0.5) / grid as f32 * sh as f32)
            .floor()
            .clamp(0.0, (sh - 1) as f32) as usize;
        let row = gy * grid;
        for gx in 0..grid {
            let sx = ((gx as f32 + 0.5) / grid as f32 * sw as f32)
                .floor()
                .clamp(0.0, (sw - 1) as f32) as usize;
            let base = (sy * sw + sx) * 4;
            let rgb = [
                image.rgba16f[base].to_f32().max(0.0),
                image.rgba16f[base + 1].to_f32().max(0.0),
                image.rgba16f[base + 2].to_f32().max(0.0),
            ];
            let (_, lab) = rgb_to_hsl_and_lab(rgb);
            lab_grid[row + gx] = lab;
        }
    }
    lab_grid
}

fn compute_auto_saliency_prob(lab_grid: &[[f32; 3]], grid: usize) -> Vec<f32> {
    let total = grid * grid;
    let border = (grid / 12).max(4);
    let mut bg_sums = [[0.0_f64; 3]; 4];
    let mut bg_cnts = [0usize; 4];

    for gy in 0..grid {
        for gx in 0..grid {
            let lab = lab_grid[gy * grid + gx];
            if gy < border {
                for ch in 0..3 {
                    bg_sums[0][ch] += lab[ch] as f64;
                }
                bg_cnts[0] += 1;
            }
            if gy >= grid - border {
                for ch in 0..3 {
                    bg_sums[1][ch] += lab[ch] as f64;
                }
                bg_cnts[1] += 1;
            }
            if gx < border {
                for ch in 0..3 {
                    bg_sums[2][ch] += lab[ch] as f64;
                }
                bg_cnts[2] += 1;
            }
            if gx >= grid - border {
                for ch in 0..3 {
                    bg_sums[3][ch] += lab[ch] as f64;
                }
                bg_cnts[3] += 1;
            }
        }
    }

    let mut bg_protos = [[0.0_f32; 3]; 4];
    for p in 0..4 {
        let inv = 1.0 / bg_cnts[p].max(1) as f64;
        bg_protos[p] = [
            (bg_sums[p][0] * inv) as f32,
            (bg_sums[p][1] * inv) as f32,
            (bg_sums[p][2] * inv) as f32,
        ];
    }

    let mut fg_sum = [0.0_f64; 3];
    let mut fg_w_sum = 0.0_f64;
    let mut max_bg_dist = 1.0_f32;
    let mut bg_dists = vec![0.0_f32; total];

    for gy in 0..grid {
        let v = (gy as f32 + 0.5) / grid as f32;
        let row = gy * grid;
        for gx in 0..grid {
            let u = (gx as f32 + 0.5) / grid as f32;
            let lab = lab_grid[row + gx];
            let mut d_min = f32::MAX;
            for proto in &bg_protos {
                let d = lab_dist(lab, *proto);
                if d < d_min {
                    d_min = d;
                }
            }
            bg_dists[row + gx] = d_min;
            if d_min > max_bg_dist {
                max_bg_dist = d_min;
            }

            if (0.22..=0.78).contains(&u) && (0.18..=0.82).contains(&v) {
                let dx = (u - 0.5) * 2.0;
                let dy = (v - 0.5) * 2.0;
                let spatial = (-1.6 * (dx * dx + dy * dy)).exp();
                let w = (d_min * d_min * spatial) as f64 + 1e-4;
                for ch in 0..3 {
                    fg_sum[ch] += lab[ch] as f64 * w;
                }
                fg_w_sum += w;
            }
        }
    }

    let fg_proto = if fg_w_sum > 1e-6 {
        [
            (fg_sum[0] / fg_w_sum) as f32,
            (fg_sum[1] / fg_w_sum) as f32,
            (fg_sum[2] / fg_w_sum) as f32,
        ]
    } else {
        lab_grid[(grid / 2) * grid + (grid / 2)]
    };

    let uniform_scene = max_bg_dist < 4.0;
    let mut prob = vec![0.0_f32; total];
    for gy in 0..grid {
        let v = (gy as f32 + 0.5) / grid as f32;
        let dy = (v - 0.5) * 2.0;
        let row = gy * grid;
        for gx in 0..grid {
            let u = (gx as f32 + 0.5) / grid as f32;
            let dx = (u - 0.5) * 2.0;
            let r2 = dx * dx + dy * dy;
            let center_prior = (-1.15 * r2).exp();

            let p = if uniform_scene {
                (1.0 - smoothstep(0.32, 0.78, r2.sqrt())).clamp(0.0, 1.0)
            } else {
                let d_bg = bg_dists[row + gx];
                let d_fg = lab_dist(lab_grid[row + gx], fg_proto);
                let color_ratio = d_bg / (d_bg + d_fg + 1e-3);
                let bg_contrast = (d_bg / (max_bg_dist * 0.45).max(8.0)).clamp(0.0, 1.0);
                let edge_dist = u.min(1.0 - u).min(v).min(1.0 - v);
                let border_fade = smoothstep(0.0, 0.06, edge_dist);
                let raw = (color_ratio * 0.62 + bg_contrast * 0.23 + center_prior * 0.15)
                    * (0.35 + 0.65 * border_fade);
                smoothstep(0.30, 0.68, raw)
            };
            prob[row + gx] = p;
        }
    }
    prob
}

fn sample_patch_lab(lab_grid: &[[f32; 3]], grid: usize, u: f32, v: f32, radius: isize) -> [f32; 3] {
    let cx =
        ((u.clamp(0.0, 1.0) * (grid - 1) as f32).round() as isize).clamp(0, (grid - 1) as isize);
    let cy =
        ((v.clamp(0.0, 1.0) * (grid - 1) as f32).round() as isize).clamp(0, (grid - 1) as isize);
    let mut sum = [0.0_f64; 3];
    let mut cnt = 0usize;
    for dy in -radius..=radius {
        let ny = (cy + dy).clamp(0, (grid - 1) as isize) as usize;
        for dx in -radius..=radius {
            let nx = (cx + dx).clamp(0, (grid - 1) as isize) as usize;
            let lab = lab_grid[ny * grid + nx];
            sum[0] += lab[0] as f64;
            sum[1] += lab[1] as f64;
            sum[2] += lab[2] as f64;
            cnt += 1;
        }
    }
    let inv = 1.0 / cnt.max(1) as f64;
    [
        (sum[0] * inv) as f32,
        (sum[1] * inv) as f32,
        (sum[2] * inv) as f32,
    ]
}

fn upsample_prob_to_raster(
    prob: &[f32],
    src_grid: usize,
    dst_grid: usize,
    invert: bool,
) -> MaskShape {
    let mut bytes = vec![0u8; dst_grid * dst_grid];
    let max_s = (src_grid - 1) as f32;
    let scale = if dst_grid > 1 {
        max_s / (dst_grid - 1) as f32
    } else {
        0.0
    };

    for gy in 0..dst_grid {
        let fy = (gy as f32 * scale).clamp(0.0, max_s);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(src_grid - 1);
        let ty = fy - y0 as f32;
        let row0 = y0 * src_grid;
        let row1 = y1 * src_grid;
        let dst_row = gy * dst_grid;

        for gx in 0..dst_grid {
            let fx = (gx as f32 * scale).clamp(0.0, max_s);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(src_grid - 1);
            let tx = fx - x0 as f32;

            let p00 = prob[row0 + x0];
            let p10 = prob[row0 + x1];
            let p01 = prob[row1 + x0];
            let p11 = prob[row1 + x1];
            let top = p00 + (p10 - p00) * tx;
            let bot = p01 + (p11 - p01) * tx;
            let v = (top + (bot - top) * ty).clamp(0.0, 1.0);
            let q = (v * 255.0).round() as u8;
            bytes[dst_row + gx] = if invert { 255 - q } else { q };
        }
    }

    MaskShape::from_raster_u8(dst_grid as u32, dst_grid as u32, &bytes)
}

#[inline]
fn lab_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dl = a[0] - b[0];
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    (dl * dl + da * da + db * db).sqrt()
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn joint_bilateral_pass(src: &[f32], lab: &[[f32; 3]], grid: usize, dst: &mut [f32]) {
    let inv_two_sigma_c2 = 1.0 / (2.0 * 14.0 * 14.0);
    const OFFSETS: [(isize, isize, f32); 9] = [
        (-2, -2, 0.35),
        (0, -2, 0.65),
        (2, -2, 0.35),
        (-2, 0, 0.65),
        (0, 0, 1.00),
        (2, 0, 0.65),
        (-2, 2, 0.35),
        (0, 2, 0.65),
        (2, 2, 0.35),
    ];

    for y in 0..grid {
        let row = y * grid;
        for x in 0..grid {
            let c_lab = lab[row + x];
            let mut sum = 0.0_f32;
            let mut w_sum = 0.0_f32;
            for &(ox, oy, sw) in &OFFSETS {
                let nx = (x as isize + ox).clamp(0, (grid - 1) as isize) as usize;
                let ny = (y as isize + oy).clamp(0, (grid - 1) as isize) as usize;
                let n_idx = ny * grid + nx;
                let n_lab = lab[n_idx];
                let dl = c_lab[0] - n_lab[0];
                let da = c_lab[1] - n_lab[1];
                let db = c_lab[2] - n_lab[2];
                let d2 = dl * dl + da * da + db * db;
                let w = sw * (-d2 * inv_two_sigma_c2).exp();
                sum += src[n_idx] * w;
                w_sum += w;
            }
            dst[row + x] = if w_sum > 1e-6 {
                (sum / w_sum).clamp(0.0, 1.0)
            } else {
                src[row + x]
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rune_editor_core::{
        LAYER_MASK_TILE_SIZE, ModelId, ModelInstallState, PromptBox, PromptPoint,
        RUNE_MODELS_DIR_ENV, SegmentationEngine, SmartMaskPrompt, inspect_model_install,
    };

    use super::*;
    use crate::load_file;
    use crate::model_manager::{
        ModelError, active_engine_summary, download_model, list_model_statuses, remove_model,
        setup_model_from_local_dir,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct ScopedModelsDir {
        dir: PathBuf,
        prev: Option<std::ffi::OsString>,
    }

    impl ScopedModelsDir {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "rune_models_test_{label}_{}_{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create temp models dir");
            let prev = std::env::var_os(RUNE_MODELS_DIR_ENV);
            // SAFETY: serialized via ENV_LOCK across tests in this crate.
            unsafe {
                std::env::set_var(RUNE_MODELS_DIR_ENV, &dir);
            }
            Self { dir, prev }
        }
    }

    impl Drop for ScopedModelsDir {
        fn drop(&mut self) {
            unsafe {
                if let Some(ref prev) = self.prev {
                    std::env::set_var(RUNE_MODELS_DIR_ENV, prev);
                } else {
                    std::env::remove_var(RUNE_MODELS_DIR_ENV);
                }
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn editorial_portrait_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("assets")
            .join("samples")
            .join("editorial_portrait.jpg")
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_vision_segments_editorial_portrait_subject_background_and_interactive() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _models = ScopedModelsDir::new("apple_vision");

        let img = load_file(&editorial_portrait_path()).expect("load editorial_portrait.jpg");

        // 1. AutoSubject via Apple Vision ANE
        let subj = generate_smart_mask(
            &img,
            &SmartMaskPrompt::AutoSubject,
            Some(SegmentationEngine::AppleVision),
        )
        .expect("Apple Vision AutoSubject");

        assert_eq!(subj.engine_used, SegmentationEngine::AppleVision);
        assert!(
            subj.instance_count.unwrap_or(0) >= 1,
            "expected at least 1 foreground instance"
        );
        let (sw, sh, subj_bytes) = subj
            .mask
            .decode_raster_u8()
            .expect("decode subject raster mask");
        assert_eq!(sw as usize, LAYER_MASK_TILE_SIZE);
        assert_eq!(sh as usize, LAYER_MASK_TILE_SIZE);

        let fg_fraction =
            subj_bytes.iter().filter(|&&b| b >= 128).count() as f32 / subj_bytes.len() as f32;
        assert!(
            (0.15..=0.85).contains(&fg_fraction),
            "expected portrait foreground coverage in 15%..85%, got {:.1}%",
            fg_fraction * 100.0
        );

        // 2. AutoBackground must be the exact complement (subject + background == 255).
        let bg = generate_smart_mask(
            &img,
            &SmartMaskPrompt::AutoBackground,
            Some(SegmentationEngine::AppleVision),
        )
        .expect("Apple Vision AutoBackground");
        let (bw, bh, bg_bytes) = bg
            .mask
            .decode_raster_u8()
            .expect("decode background raster mask");
        assert_eq!((bw, bh), (sw, sh));
        for (i, (&s, &b)) in subj_bytes.iter().zip(bg_bytes.iter()).enumerate() {
            assert_eq!(
                s as u16 + b as u16,
                255,
                "subject+bg mismatch at pixel {i}: s={s}, b={b}"
            );
        }

        // 3. Verify Y-axis orientation & interactive point selection on center/upper face.
        // Pick a known foreground pixel near the center/upper portrait region and verify clicking it
        // selects the person instance with strong alpha at the clicked coordinate.
        let click_u = 0.50_f32;
        let click_v = 0.42_f32;
        let interactive = generate_smart_mask(
            &img,
            &SmartMaskPrompt::Interactive {
                points: vec![PromptPoint::new(click_u, click_v, true)],
                bbox: None,
                invert: false,
            },
            Some(SegmentationEngine::AppleVision),
        )
        .expect("Apple Vision Interactive");
        assert_eq!(interactive.engine_used, SegmentationEngine::AppleVision);
        let (iw, ih, inter_bytes) = interactive
            .mask
            .decode_raster_u8()
            .expect("decode interactive mask");
        assert_eq!(
            (iw as usize, ih as usize),
            (LAYER_MASK_TILE_SIZE, LAYER_MASK_TILE_SIZE)
        );

        let inter_cov =
            inter_bytes.iter().filter(|&&b| b >= 128).count() as f32 / inter_bytes.len() as f32;
        assert!(
            (0.15..=0.85).contains(&inter_cov),
            "expected interactive click on portrait subject coverage in 15%..85%, got {:.1}%",
            inter_cov * 100.0
        );

        let gx = ((click_u * (iw - 1) as f32).round() as usize).min(iw as usize - 1);
        let gy = ((click_v * (ih - 1) as f32).round() as usize).min(ih as usize - 1);
        let clicked_alpha = inter_bytes[gy * iw as usize + gx];
        assert!(
            clicked_alpha >= 180,
            "expected clicked portrait point ({click_u}, {click_v}) to be foreground (>=180), got {clicked_alpha}"
        );

        // Verify top-of-frame sky/backdrop corner (0.04, 0.04) is background while lower torso (0.50, 0.85) is foreground
        // (confirms top-down image Y matches mask Y without vertical flip).
        let corner_alpha = inter_bytes
            [(0.04 * (ih - 1) as f32) as usize * iw as usize + (0.04 * (iw - 1) as f32) as usize];
        let torso_alpha = inter_bytes
            [(0.85 * (ih - 1) as f32) as usize * iw as usize + (0.50 * (iw - 1) as f32) as usize];
        assert!(
            torso_alpha > corner_alpha,
            "expected lower torso ({torso_alpha}) > top-left backdrop ({corner_alpha}) confirming upright Y orientation"
        );
    }

    #[test]
    fn model_manager_enforces_sam3_license_and_local_setup_remove_lifecycle() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let scoped = ScopedModelsDir::new("lifecycle");

        // 1. SAM 3 license gate must reject download & local setup without accept_license=true.
        for sam3 in [ModelId::Sam3TrackerQ4f16, ModelId::Sam3TrackerFp16] {
            let err = download_model(sam3, false, None, |_| {})
                .expect_err("SAM 3 download must fail without license acceptance");
            match err {
                ModelError::LicenseRequired {
                    model_id,
                    license,
                    license_url,
                } => {
                    assert_eq!(model_id, sam3);
                    assert!(license.contains("Meta SAM"));
                    assert!(license_url.starts_with("https://"));
                }
                other => panic!("expected LicenseRequired, got {other:?}"),
            }

            let setup_err = setup_model_from_local_dir(sam3, &scoped.dir, false)
                .expect_err("SAM 3 local setup must fail without license acceptance");
            assert!(matches!(setup_err, ModelError::LicenseRequired { .. }));
        }

        // 2. Prepare synthetic offline source directory matching Hugging Face `onnx/` layout.
        let offline_src = scoped.dir.join("offline_src");
        let onnx_sub = offline_src.join("onnx");
        fs::create_dir_all(&onnx_sub).expect("create offline_src/onnx");
        for file in ModelId::Sam3TrackerQ4f16.spec().files {
            fs::write(onnx_sub.join(file.filename), b"DUMMY_ONNX_PAYLOAD_123456")
                .expect("write dummy model file");
        }

        assert_eq!(
            inspect_model_install(ModelId::Sam3TrackerQ4f16),
            ModelInstallState::NotInstalled
        );

        let status = setup_model_from_local_dir(ModelId::Sam3TrackerQ4f16, &offline_src, true)
            .expect("setup SAM 3 from local dir with license acceptance");
        assert!(status.state.is_installed());
        assert!(status.license_accepted);
        assert!(inspect_model_install(ModelId::Sam3TrackerQ4f16).is_installed());

        let statuses = list_model_statuses();
        assert_eq!(statuses.len(), ModelId::ALL.len());
        let sam3_status = statuses
            .iter()
            .find(|s| s.id == ModelId::Sam3TrackerQ4f16)
            .expect("SAM 3 status present");
        assert!(sam3_status.state.is_installed());
        assert!(sam3_status.license_accepted);

        let summary = active_engine_summary();
        assert!(
            summary
                .installed_models
                .contains(&ModelId::Sam3TrackerQ4f16)
        );
        assert_eq!(
            summary.effective_interactive_engine,
            SegmentationEngine::Sam3Tracker
        );

        // 3. Remove model cleanly and confirm state returns to NotInstalled.
        remove_model(ModelId::Sam3TrackerQ4f16).expect("remove SAM 3");
        assert_eq!(
            inspect_model_install(ModelId::Sam3TrackerQ4f16),
            ModelInstallState::NotInstalled
        );
    }

    #[test]
    fn auto_fallback_and_interactive_heuristic_work_without_downloaded_models() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _models = ScopedModelsDir::new("fallback");

        let img = load_file(&editorial_portrait_path()).expect("load editorial_portrait.jpg");

        // Explicit SAM 3 override must report ModelNotInstalled when not downloaded.
        let err = generate_smart_mask(
            &img,
            &SmartMaskPrompt::AutoSubject,
            Some(SegmentationEngine::Sam3Tracker),
        )
        .expect_err("explicit SAM 3 should fail when not installed");
        assert!(matches!(
            err,
            VisionError::ModelNotInstalled(ModelId::Sam3TrackerQ4f16)
        ));

        // Auto mode succeeds cleanly without any downloaded models (Apple Vision on macOS, Heuristic elsewhere).
        let auto_res = generate_smart_mask(&img, &SmartMaskPrompt::AutoSubject, None)
            .expect("Auto mode should always succeed");
        #[cfg(target_os = "macos")]
        assert_eq!(auto_res.engine_used, SegmentationEngine::AppleVision);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(auto_res.engine_used, SegmentationEngine::Heuristic);

        let (w, h, bytes) = auto_res.mask.decode_raster_u8().expect("decode auto mask");
        assert_eq!(
            (w as usize, h as usize),
            (LAYER_MASK_TILE_SIZE, LAYER_MASK_TILE_SIZE)
        );
        assert!(bytes.iter().any(|&b| b > 200));

        // Interactive heuristic fallback handles point + box prompts cleanly at 512×512.
        let heur = generate_smart_mask(
            &img,
            &SmartMaskPrompt::Interactive {
                points: vec![
                    PromptPoint::new(0.5, 0.45, true),
                    PromptPoint::new(0.05, 0.05, false),
                ],
                bbox: Some(PromptBox::new(0.2, 0.1, 0.8, 0.95)),
                invert: false,
            },
            Some(SegmentationEngine::Heuristic),
        )
        .expect("interactive heuristic");
        assert_eq!(heur.engine_used, SegmentationEngine::Heuristic);
        let (hw, hh, hbytes) = heur.mask.decode_raster_u8().expect("decode heuristic mask");
        assert_eq!(
            (hw as usize, hh as usize),
            (LAYER_MASK_TILE_SIZE, LAYER_MASK_TILE_SIZE)
        );
        let cov = hbytes.iter().filter(|&&b| b >= 128).count() as f32 / hbytes.len() as f32;
        assert!(
            (0.05..=0.90).contains(&cov),
            "expected reasonable interactive heuristic coverage, got {:.1}%",
            cov * 100.0
        );
    }

    #[test]
    fn sam_preprocessing_and_fingerprint_are_deterministic() {
        let img = crate::generate_demo_hdr_scene(120, 80);
        let fp1 = crate::sam_ort::compute_image_fingerprint(&img);
        let fp2 = crate::sam_ort::compute_image_fingerprint(&img);
        assert_eq!(fp1, fp2);

        let mut modified = img.clone();
        modified.rgba16f[0] = half::f16::from_f32(0.99);
        let fp3 = crate::sam_ort::compute_image_fingerprint(&modified);
        assert_ne!(fp1, fp3);

        let spec = ModelId::Sam21HieraTinyFp16.spec();
        let tensor = crate::sam_ort::preprocess_image_nchw_f32(&img, spec);
        assert_eq!(
            tensor.len(),
            3 * (spec.input_size as usize) * (spec.input_size as usize)
        );
        assert!(tensor.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn model_manager_respects_cancel_flag_and_rejects_html_payloads() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let scoped = ScopedModelsDir::new("cancel_and_html");

        // Pre-cancelled flag must immediately return Cancelled without network activity.
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let err = download_model(ModelId::Sam21HieraTinyFp16, false, Some(cancel), |_| {})
            .expect_err("pre-cancelled download must fail with Cancelled");
        assert!(matches!(
            err,
            ModelError::Cancelled(ModelId::Sam21HieraTinyFp16)
        ));

        // HTML error page masquerading as an ONNX file must be rejected cleanly before any copy.
        let bad_src = scoped.dir.join("bad_html");
        fs::create_dir_all(&bad_src).expect("create bad_html dir");
        let files = ModelId::Sam21HieraTinyFp16.spec().files;
        for (idx, file) in files.iter().enumerate() {
            if idx + 1 == files.len() {
                fs::write(
                    bad_src.join(file.filename),
                    b"<!DOCTYPE html><html><body>404 Not Found</body></html>",
                )
                .expect("write html stub");
            } else {
                fs::write(bad_src.join(file.filename), b"VALID_ONNX_BYTES")
                    .expect("write valid stub");
            }
        }
        let err = setup_model_from_local_dir(ModelId::Sam21HieraTinyFp16, &bad_src, false)
            .expect_err("HTML payload must be rejected");
        assert!(matches!(err, ModelError::EmptyOrCorruptFile { .. }));
        // Verify pre-validation left zero partial files in destination directory.
        assert_eq!(
            inspect_model_install(ModelId::Sam21HieraTinyFp16),
            ModelInstallState::NotInstalled
        );
    }

    #[test]
    fn truncated_linear_image_never_panics_and_broken_symlink_model_dir_is_rejected() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let scoped = ScopedModelsDir::new("truncated_and_symlink");

        // 1. Truncated LinearImage (width*height*4 > rgba16f.len()) must fail cleanly without panic.
        let truncated = LinearImage {
            width: 128,
            height: 128,
            rgba16f: vec![half::f16::ZERO; 16],
            histogram_proxy: Vec::new(),
            display_name: "truncated.dng".to_owned(),
            source_path: None,
            format_label: "RAW".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        assert!(matches!(
            generate_smart_mask(&truncated, &SmartMaskPrompt::AutoSubject, None),
            Err(VisionError::EmptyImage)
        ));
        let heur = generate_heuristic_mask(&truncated, false);
        assert!(heur.decode_raster_u8().is_some());
        let inter = generate_interactive_heuristic_mask(
            &truncated,
            &[PromptPoint::new(0.5, 0.5, true)],
            None,
            false,
        );
        assert!(inter.decode_raster_u8().is_some());

        // 2. Broken symlink at model_dir(id) must be rejected by remove_model & setup_model_from_local_dir.
        #[cfg(unix)]
        {
            let target_dir = rune_editor_core::model_dir(ModelId::Sam21HieraTinyFp16);
            std::os::unix::fs::symlink(scoped.dir.join("nonexistent_target"), &target_dir)
                .expect("create broken symlink");
            let err = remove_model(ModelId::Sam21HieraTinyFp16)
                .expect_err("remove_model must reject broken symlink");
            assert!(matches!(err, ModelError::InvalidPath(_)));
            let _ = fs::remove_file(&target_dir);
        }
    }
}
