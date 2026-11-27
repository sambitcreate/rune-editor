//! ONNX Runtime segmentation backend supporting Meta SAM 2.1 Hiera Tiny (`fp16`) and
//! Meta SAM 3 Tracker (`q4f16` / `fp16`) with CoreML EP acceleration on macOS + graceful CPU fallback.
//!
//! Features:
//! - Global `Mutex<Option<SamSessionCache>>` caching loaded encoder/decoder sessions by [`ModelId`]
//!   AND caching the last encoded image's feature pyramid by `(ModelId, image_fingerprint)` so
//!   interactive point/box clicks re-run only the tiny mask decoder (~10–25ms).
//! - Dynamic tensor introspection matching encoder outputs to decoder inputs by name or position,
//!   converting between `f16` and `f32` automatically, and adapting to rank-3 vs rank-4 prompt
//!   tensors across SAM 2.1 and SAM 3 ONNX exports.

use std::borrow::Cow;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use half::f16;
use ort::environment::Environment;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::{Tensor, TensorElementType, ValueType};
use rune_editor_core::{
    LAYER_MASK_TILE_SIZE, MaskShape, ModelId, ModelSpec, PromptBox, PromptPoint, SmartMaskPrompt,
    inspect_model_install, linear_to_srgb, model_decoder_path, model_encoder_path,
    rgb_to_hsl_and_lab,
};

use crate::LinearImage;

/// Structured output returned by ONNX Runtime SAM inference.
#[derive(Clone, Debug)]
pub struct SamMaskResult {
    pub mask: MaskShape,
    pub iou_score: f32,
    pub model_id: ModelId,
    pub backend_label: &'static str,
}

#[derive(Clone)]
enum CachedTensorData {
    F32(Vec<f32>),
    F16(Vec<f16>),
}

#[derive(Clone)]
struct CachedTensor {
    name: String,
    shape: Vec<i64>,
    data: CachedTensorData,
}

struct CachedImageEmbeddings {
    model_id: ModelId,
    image_fingerprint: u64,
    tensors: Vec<CachedTensor>,
}

struct SamSessionCache {
    model_id: ModelId,
    encoder: Session,
    decoder: Session,
    encoder_used_coreml: bool,
    decoder_used_coreml: bool,
    backend_label: &'static str,
    last_encoded: Option<CachedImageEmbeddings>,
}

static ORT_ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
static SESSION_CACHE: Mutex<Option<SamSessionCache>> = Mutex::new(None);

fn ort_environment() -> Result<&'static Environment, String> {
    match ORT_ENV.get_or_init(|| {
        ort::init()
            .with_name("rune-darkroom-sam")
            .build()
            .map_err(|e| format!("Failed to initialize ONNX Runtime environment: {e}"))
    }) {
        Ok(env) => Ok(env),
        Err(msg) => Err(msg.clone()),
    }
}

fn lock_session_cache() -> std::sync::MutexGuard<'static, Option<SamSessionCache>> {
    SESSION_CACHE.lock().unwrap_or_else(|poisoned| {
        let mut guard = poisoned.into_inner();
        *guard = None;
        guard
    })
}

/// Evicts any cached ONNX Runtime SAM sessions and feature pyramids (e.g. after model removal).
pub fn evict_session_cache() {
    let mut guard = lock_session_cache();
    *guard = None;
}

/// Evicts the cached session only if it currently holds `id`.
pub fn evict_model_from_cache(id: ModelId) {
    let mut guard = lock_session_cache();
    if guard.as_ref().is_some_and(|c| c.model_id == id) {
        *guard = None;
    }
}

/// Runs SAM 2.1 or SAM 3 segmentation on `image` using an installed ONNX model `model_id`.
pub fn segment(
    image: &LinearImage,
    prompt: &SmartMaskPrompt,
    model_id: ModelId,
) -> Result<SamMaskResult, String> {
    if !image.has_valid_buffer() {
        return Err("Cannot run SAM segmentation on an empty or truncated image buffer".to_owned());
    }
    if !inspect_model_install(model_id).is_installed() {
        return Err(format!(
            "Model '{}' is not installed. Run model setup/download first.",
            model_id.id()
        ));
    }

    let prompt = prompt.sanitized();
    let spec = model_id.spec();
    let fingerprint = compute_image_fingerprint(image);

    // Prepare seed points for AutoSubject/AutoBackground outside the cache lock so any
    // optional Apple Vision centroid lookup never blocks the SAM mutex.
    let (points, bbox, invert) = resolve_prompt_for_sam(image, &prompt);

    let mut guard = lock_session_cache();

    // Load or switch model sessions if needed.
    // Note: Apple CoreML EP in ORT 2.0-rc.15 crashes (`SIGABRT` in MPSGraphExecutable.mm:4419
    // with ComputeUnits::All) or fails shape inference (`model.mm:79 GetStaticOutputShape`) after
    // ~20s of graph compilation on dynamic-shape SAM 2.1 / SAM 3 Hiera graphs. Default to the
    // multi-threaded CPU EP (opt-in CoreML via `RUNE_SAM_COREML=1` for experimentation).
    let try_coreml = std::env::var("RUNE_SAM_COREML").is_ok_and(|v| v == "1" || v == "true");
    let needs_load = guard
        .as_ref()
        .is_none_or(|cache| cache.model_id != model_id);
    if needs_load {
        let loaded = load_model_sessions(model_id, try_coreml)?;
        *guard = Some(loaded);
    }

    let cache = guard
        .as_mut()
        .ok_or_else(|| "SAM session cache unexpectedly empty".to_owned())?;

    // Step 1: Run vision encoder (or hit feature-pyramid cache for instant interactive clicks).
    let embeddings_cached = cache
        .last_encoded
        .as_ref()
        .is_some_and(|e| e.model_id == model_id && e.image_fingerprint == fingerprint);

    if !embeddings_cached {
        match run_vision_encoder(&mut cache.encoder, image, spec) {
            Ok(tensors) => {
                cache.last_encoded = Some(CachedImageEmbeddings {
                    model_id,
                    image_fingerprint: fingerprint,
                    tensors,
                });
            }
            Err(enc_err) if cache.encoder_used_coreml => {
                // CoreML EP occasionally rejects a quantized operator at runtime (e.g. q4f16);
                // rebuild cleanly on CPU EP and retry once.
                let cpu_cache = load_model_sessions(model_id, false).map_err(|cpu_err| {
                    format!(
                        "CoreML encoder failed ({enc_err}); CPU fallback load failed ({cpu_err})"
                    )
                })?;
                *cache = cpu_cache;
                let tensors = run_vision_encoder(&mut cache.encoder, image, spec)?;
                cache.last_encoded = Some(CachedImageEmbeddings {
                    model_id,
                    image_fingerprint: fingerprint,
                    tensors,
                });
            }
            Err(enc_err) => return Err(enc_err),
        }
    }

    let encoded_tensors = cache
        .last_encoded
        .as_ref()
        .ok_or_else(|| "Missing cached SAM image embeddings".to_owned())?
        .tensors
        .clone();

    // Step 2: Run prompt encoder + mask decoder.
    let (mask, iou_score) = match run_mask_decoder(
        &mut cache.decoder,
        image,
        &encoded_tensors,
        spec,
        &points,
        bbox.as_ref(),
        invert,
    ) {
        Ok(res) => res,
        Err(dec_err) if cache.decoder_used_coreml => {
            // Rebuild ONLY the mask decoder on CPU EP and reuse the cached vision-encoder pyramid!
            let env = ort_environment()?;
            let dec_path = model_decoder_path(model_id);
            let (cpu_decoder, _) = build_session(env, &dec_path, false).map_err(|cpu_err| {
                format!(
                    "CoreML decoder failed ({dec_err}); CPU decoder fallback load failed ({cpu_err})"
                )
            })?;
            cache.decoder = cpu_decoder;
            cache.decoder_used_coreml = false;
            cache.backend_label = sam_backend_label(model_id, cache.encoder_used_coreml, false);
            run_mask_decoder(
                &mut cache.decoder,
                image,
                &encoded_tensors,
                spec,
                &points,
                bbox.as_ref(),
                invert,
            )?
        }
        Err(dec_err) => return Err(dec_err),
    };

    Ok(SamMaskResult {
        mask,
        iou_score,
        model_id,
        backend_label: cache.backend_label,
    })
}

fn sam_backend_label(
    model_id: ModelId,
    encoder_coreml: bool,
    decoder_coreml: bool,
) -> &'static str {
    let any_coreml = encoder_coreml || decoder_coreml;
    match (model_id, any_coreml) {
        (ModelId::Sam21HieraTinyFp16, true) => "SAM 2.1 Hiera Tiny (CoreML EP)",
        (ModelId::Sam21HieraTinyFp16, false) => "SAM 2.1 Hiera Tiny (CPU EP)",
        (ModelId::Sam3TrackerQ4f16, true) => "SAM 3 Tracker Q4F16 (CoreML EP)",
        (ModelId::Sam3TrackerQ4f16, false) => "SAM 3 Tracker Q4F16 (CPU EP)",
        (ModelId::Sam3TrackerFp16, true) => "SAM 3 Tracker FP16 (CoreML EP)",
        (ModelId::Sam3TrackerFp16, false) => "SAM 3 Tracker FP16 (CPU EP)",
    }
}

fn load_model_sessions(model_id: ModelId, try_coreml: bool) -> Result<SamSessionCache, String> {
    let env = ort_environment()?;
    let enc_path = model_encoder_path(model_id);
    let dec_path = model_decoder_path(model_id);

    if !enc_path.is_file() {
        return Err(format!(
            "SAM encoder ONNX file missing at '{}'",
            enc_path.display()
        ));
    }
    if !dec_path.is_file() {
        return Err(format!(
            "SAM decoder ONNX file missing at '{}'",
            dec_path.display()
        ));
    }

    let (encoder, encoder_used_coreml) = build_session(env, &enc_path, try_coreml)?;
    let (decoder, decoder_used_coreml) =
        build_session(env, &dec_path, try_coreml && encoder_used_coreml)?;
    let backend_label = sam_backend_label(model_id, encoder_used_coreml, decoder_used_coreml);

    Ok(SamSessionCache {
        model_id,
        encoder,
        decoder,
        encoder_used_coreml,
        decoder_used_coreml,
        backend_label,
        last_encoded: None,
    })
}

fn build_session(
    env: &Environment,
    path: &Path,
    try_coreml: bool,
) -> Result<(Session, bool), String> {
    #[cfg(target_os = "macos")]
    if try_coreml {
        let ep = ort::ep::CoreML::default()
            .with_static_input_shapes(true)
            .with_compute_units(ort::ep::coreml::ComputeUnits::CPUAndNeuralEngine)
            .with_model_format(ort::ep::coreml::ModelFormat::MLProgram)
            .build();
        if let Ok(builder) = Session::builder(env)
            && let Ok(b) = builder.with_optimization_level(GraphOptimizationLevel::Level3)
            && let Ok(mut b) = b.with_execution_providers([ep])
            && let Ok(session) = b.commit_from_file(path)
        {
            return Ok((session, true));
        }
    }

    #[cfg(not(target_os = "macos"))]
    let _ = try_coreml;

    let threads = std::thread::available_parallelism()
        .map(|n| n.get().clamp(1, 8))
        .unwrap_or(4);
    let mut builder = Session::builder(env)
        .map_err(|e| format!("Failed to create ORT SessionBuilder: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| format!("Failed to set ORT optimization level: {e}"))?
        .with_intra_threads(threads)
        .map_err(|e| format!("Failed to set ORT intra-op threads: {e}"))?;

    let session = builder
        .commit_from_file(path)
        .map_err(|e| format!("Failed to load ONNX model '{}': {e}", path.display()))?;
    Ok((session, false))
}

fn run_vision_encoder(
    encoder: &mut Session,
    image: &LinearImage,
    spec: &ModelSpec,
) -> Result<Vec<CachedTensor>, String> {
    let input_outlet = encoder
        .inputs()
        .first()
        .ok_or_else(|| "SAM vision encoder has no inputs".to_owned())?;
    let input_name = input_outlet.name().to_owned();
    let expected_ty = match input_outlet.dtype() {
        ValueType::Tensor { ty, .. } => *ty,
        _ => TensorElementType::Float32,
    };

    let size = spec.input_size as usize;
    let planar_f32 = preprocess_image_nchw_f32(image, spec);

    let input_val: SessionInputValue<'static> = match expected_ty {
        TensorElementType::Float16 => {
            let f16_data: Vec<f16> = planar_f32.into_iter().map(f16::from_f32).collect();
            Tensor::from_array(([1usize, 3, size, size], f16_data))
                .map_err(|e| format!("Failed to build f16 encoder input tensor: {e}"))?
                .into()
        }
        _ => Tensor::from_array(([1usize, 3, size, size], planar_f32))
            .map_err(|e| format!("Failed to build f32 encoder input tensor: {e}"))?
            .into(),
    };

    let inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> =
        vec![(Cow::Owned(input_name), input_val)];
    let outputs = encoder
        .run(inputs)
        .map_err(|e| format!("SAM vision encoder inference failed: {e}"))?;

    let mut cached = Vec::with_capacity(outputs.len());
    for (name, val) in outputs.iter() {
        let dtype = match val.dtype() {
            ValueType::Tensor { ty, .. } => *ty,
            _ => continue,
        };
        match dtype {
            TensorElementType::Float16 => {
                let (shape, slice) = val
                    .try_extract_tensor::<f16>()
                    .map_err(|e| format!("Failed to extract f16 encoder output '{name}': {e}"))?;
                cached.push(CachedTensor {
                    name: name.to_owned(),
                    shape: shape.iter().copied().collect(),
                    data: CachedTensorData::F16(slice.to_vec()),
                });
            }
            TensorElementType::Float32 => {
                let (shape, slice) = val
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("Failed to extract f32 encoder output '{name}': {e}"))?;
                cached.push(CachedTensor {
                    name: name.to_owned(),
                    shape: shape.iter().copied().collect(),
                    data: CachedTensorData::F32(slice.to_vec()),
                });
            }
            other => {
                return Err(format!(
                    "Unsupported encoder output '{name}' element type {other}"
                ));
            }
        }
    }

    if cached.is_empty() {
        return Err("SAM vision encoder produced no tensor outputs".to_owned());
    }
    Ok(cached)
}

#[allow(clippy::too_many_arguments)]
fn run_mask_decoder(
    decoder: &mut Session,
    image: &LinearImage,
    encoded: &[CachedTensor],
    spec: &ModelSpec,
    points: &[PromptPoint],
    bbox: Option<&PromptBox>,
    invert: bool,
) -> Result<(MaskShape, f32), String> {
    let input_specs: Vec<(String, TensorElementType, Vec<i64>)> = decoder
        .inputs()
        .iter()
        .map(|outlet| {
            let (ty, shape) = match outlet.dtype() {
                ValueType::Tensor { ty, shape, .. } => (*ty, shape.iter().copied().collect()),
                _ => (TensorElementType::Float32, Vec::new()),
            };
            (outlet.name().to_owned(), ty, shape)
        })
        .collect();

    let has_dedicated_boxes_input = input_specs.iter().any(|(name, _, _)| {
        let l = name.to_ascii_lowercase();
        l.contains("box") && !l.contains("embed")
    });

    let scale = spec.input_size as f32;
    let mut coords: Vec<f32> = Vec::new();
    let mut labels: Vec<i64> = Vec::new();

    for pt in points {
        coords.push(pt.x.clamp(0.0, 1.0) * scale);
        coords.push(pt.y.clamp(0.0, 1.0) * scale);
        labels.push(if pt.positive { 1 } else { 0 });
    }

    if let Some(b) = bbox
        && !has_dedicated_boxes_input
    {
        // Standard SAM prompt convention: box encoded as two points with labels 2 (TL) and 3 (BR).
        coords.push(b.x0.clamp(0.0, 1.0) * scale);
        coords.push(b.y0.clamp(0.0, 1.0) * scale);
        labels.push(2);
        coords.push(b.x1.clamp(0.0, 1.0) * scale);
        coords.push(b.y1.clamp(0.0, 1.0) * scale);
        labels.push(3);
    }

    // When a dedicated `input_boxes` tensor is present and no points were supplied, Hugging Face
    // SAM 2.1 / SAM 3 ONNX decoders expect empty `input_points` `[1, 1, 0, 2]` and `input_labels`
    // `[1, 1, 0]` so the box alone drives the mask. Only fall back to a center point if neither
    // points nor a bounding box were provided.
    if labels.is_empty() && !(bbox.is_some() && has_dedicated_boxes_input) {
        coords.push(0.5 * scale);
        coords.push(0.5 * scale);
        labels.push(1);
    }

    let num_points = labels.len();
    let mut decoder_inputs: Vec<(Cow<'_, str>, SessionInputValue<'static>)> =
        Vec::with_capacity(input_specs.len());
    let mut positional_emb_idx = 0usize;

    for (name, expected_ty, expected_shape) in &input_specs {
        let lower = name.to_ascii_lowercase();
        let rank = expected_shape.len();

        let val: SessionInputValue<'static> = if lower.contains("point") && !lower.contains("label")
        {
            let shape: Vec<i64> = match rank {
                4 => vec![1, 1, num_points as i64, 2],
                3 => vec![1, num_points as i64, 2],
                2 => vec![num_points as i64, 2],
                _ => vec![1, 1, num_points as i64, 2],
            };
            build_float_tensor(shape, &coords, *expected_ty)?
        } else if lower.contains("label") {
            let shape: Vec<i64> = match rank {
                3 => vec![1, 1, num_points as i64],
                2 => vec![1, num_points as i64],
                1 => vec![num_points as i64],
                _ => vec![1, 1, num_points as i64],
            };
            build_label_tensor(shape, &labels, *expected_ty)?
        } else if lower.contains("box") {
            let (box_shape, box_data) = if let Some(b) = bbox {
                let data = vec![
                    b.x0.clamp(0.0, 1.0) * scale,
                    b.y0.clamp(0.0, 1.0) * scale,
                    b.x1.clamp(0.0, 1.0) * scale,
                    b.y1.clamp(0.0, 1.0) * scale,
                ];
                let shape = match rank {
                    3 => vec![1, 1, 4],
                    2 => vec![1, 4],
                    _ => vec![1, 1, 4],
                };
                (shape, data)
            } else {
                let shape = match rank {
                    3 => vec![1, 0, 4],
                    2 => vec![0, 4],
                    _ => vec![1, 0, 4],
                };
                (shape, Vec::new())
            };
            build_float_tensor(box_shape, &box_data, *expected_ty)?
        } else if lower.contains("has_mask") {
            let shape = if rank == 0 { vec![] } else { vec![1] };
            build_float_tensor(shape, &[0.0], *expected_ty)?
        } else if lower.contains("mask_input")
            || lower == "input_masks"
            || (lower.contains("mask") && lower.contains("input"))
        {
            let ms = spec.mask_size as i64;
            let shape = match rank {
                4 => vec![1, 1, ms, ms],
                3 => vec![1, ms, ms],
                _ => vec![1, 1, ms, ms],
            };
            let n = (ms as usize) * (ms as usize);
            build_float_tensor(shape, &vec![0.0_f32; n], *expected_ty)?
        } else if lower.contains("orig_im_size") {
            let shape = vec![2i64];
            match expected_ty {
                TensorElementType::Int64 => Tensor::from_array((
                    shape,
                    vec![spec.input_size as i64, spec.input_size as i64],
                ))
                .map_err(|e| format!("Failed to build orig_im_size i64 tensor: {e}"))?
                .into(),
                TensorElementType::Int32 => Tensor::from_array((
                    shape,
                    vec![spec.input_size as i32, spec.input_size as i32],
                ))
                .map_err(|e| format!("Failed to build orig_im_size i32 tensor: {e}"))?
                .into(),
                _ => build_float_tensor(shape, &[scale, scale], *expected_ty)?,
            }
        } else {
            // Match encoder feature pyramid output by exact name, suffix index, or positional order.
            let matched = encoded
                .iter()
                .find(|t| t.name == *name)
                .or_else(|| {
                    // Match trailing index like `image_embeddings.0` <-> `image_embed.0`
                    let idx_suffix = name.rsplit('.').next()?;
                    encoded
                        .iter()
                        .find(|t| t.name.rsplit('.').next() == Some(idx_suffix))
                })
                .or_else(|| encoded.get(positional_emb_idx));
            positional_emb_idx += 1;

            let tensor = matched.ok_or_else(|| {
                format!("Could not match SAM decoder input '{name}' to any encoder output")
            })?;
            build_cached_tensor_input(tensor, *expected_ty)?
        };

        decoder_inputs.push((Cow::Owned(name.clone()), val));
    }

    let outputs = decoder
        .run(decoder_inputs)
        .map_err(|e| format!("SAM mask decoder inference failed: {e}"))?;

    // Locate `pred_masks` and `iou_scores` outputs dynamically.
    let (mask_name, mask_val) = outputs
        .iter()
        .find(|(k, _)| {
            let l = k.to_ascii_lowercase();
            l == "pred_masks" || l == "masks" || l == "low_res_masks"
        })
        .or_else(|| {
            outputs
                .iter()
                .find(|(k, _)| k.to_ascii_lowercase().contains("mask"))
        })
        .or_else(|| outputs.iter().next())
        .ok_or_else(|| "SAM decoder returned no mask output".to_owned())?;

    let (mask_shape, mask_logits) = extract_tensor_as_f32(&mask_val, mask_name)?;

    // Prefer exact `iou_scores` / `*iou*` over `object_score_logits` (both exist on SAM 2.1 & SAM 3).
    let iou_output = outputs
        .iter()
        .find(|(k, _)| {
            let l = k.to_ascii_lowercase();
            l == "iou_scores" || l.contains("iou")
        })
        .or_else(|| {
            outputs.iter().find(|(k, _)| {
                let l = k.to_ascii_lowercase();
                l.contains("score") && !l.contains("object")
            })
        });
    let iou_scores: Vec<f32> = iou_output
        .and_then(|(k, v)| extract_tensor_as_f32(&v, k).ok())
        .map(|(_, data)| data)
        .unwrap_or_default();

    let (mask_h, mask_w, best_idx, best_iou) =
        select_best_mask_proposal(&mask_shape, &mask_logits, &iou_scores)?;

    let plane_len = mask_h * mask_w;
    let offset = best_idx * plane_len;
    if offset + plane_len > mask_logits.len() {
        return Err("SAM decoder pred_masks slice out of bounds".to_owned());
    }

    let mut raw_logits = vec![-20.0_f32; plane_len];
    for (dst, &logit) in raw_logits
        .iter_mut()
        .zip(&mask_logits[offset..offset + plane_len])
    {
        *dst = if logit.is_finite() {
            logit.clamp(-20.0, 20.0)
        } else {
            -20.0
        };
    }

    // Refine raw decoder logits with an image-guided period-4-nulling joint bilateral filter
    // (eliminating SAM 2.1 / SAM 3 ConvTranspose2d stride-2/stride-4 checkerboard ripples while
    // snapping silhouettes cleanly to true photo edges), then bilinearly upsample in logit space
    // and apply a crisp anti-aliased sigmoid around `logit = 0.0`.
    let clean_logits = refine_sam_logits_guided(&raw_logits, mask_w, mask_h, image);
    let mask = upsample_logits_to_raster_mask(&clean_logits, mask_w, mask_h, invert);
    Ok((mask, best_iou))
}

fn select_best_mask_proposal(
    mask_shape: &[i64],
    mask_logits: &[f32],
    iou_scores: &[f32],
) -> Result<(usize, usize, usize, f32), String> {
    if mask_shape.len() < 2 {
        return Err(format!(
            "SAM decoder mask output has invalid rank: {mask_shape:?}"
        ));
    }
    let mask_h = mask_shape[mask_shape.len() - 2].max(0) as usize;
    let mask_w = mask_shape[mask_shape.len() - 1].max(0) as usize;
    let plane_len = mask_h.saturating_mul(mask_w);
    if plane_len == 0 || mask_logits.len() < plane_len {
        return Err(format!(
            "SAM decoder mask output is empty (shape={mask_shape:?}, len={})",
            mask_logits.len()
        ));
    }

    let num_masks = (mask_logits.len() / plane_len).max(1);
    let mut best_idx = 0usize;
    let mut best_iou = 0.0_f32;

    if !iou_scores.is_empty() {
        let count = num_masks.min(iou_scores.len());
        best_iou = f32::NEG_INFINITY;
        for (i, &score) in iou_scores.iter().take(count).enumerate() {
            let s = if score.is_finite() { score } else { 0.0 };
            if s > best_iou {
                best_iou = s;
                best_idx = i;
            }
        }
        if !best_iou.is_finite() {
            best_iou = 0.0;
        }
    }

    Ok((mask_h, mask_w, best_idx, best_iou.clamp(0.0, 1.0)))
}

fn extract_tensor_as_f32(
    val: &ort::value::ValueRef<'_>,
    name: &str,
) -> Result<(Vec<i64>, Vec<f32>), String> {
    match val.dtype() {
        ValueType::Tensor {
            ty: TensorElementType::Float16,
            ..
        } => {
            let (shape, slice) = val
                .try_extract_tensor::<f16>()
                .map_err(|e| format!("Failed to extract f16 output '{name}': {e}"))?;
            Ok((
                shape.iter().copied().collect(),
                slice.iter().map(|h| h.to_f32()).collect(),
            ))
        }
        ValueType::Tensor {
            ty: TensorElementType::Float32,
            ..
        } => {
            let (shape, slice) = val
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("Failed to extract f32 output '{name}': {e}"))?;
            Ok((shape.iter().copied().collect(), slice.to_vec()))
        }
        other => Err(format!(
            "Unsupported SAM decoder output '{name}' dtype: {other:?}"
        )),
    }
}

fn build_float_tensor(
    shape: Vec<i64>,
    data: &[f32],
    ty: TensorElementType,
) -> Result<SessionInputValue<'static>, String> {
    match ty {
        TensorElementType::Float16 => {
            let vec: Vec<f16> = data.iter().map(|&v| f16::from_f32(v)).collect();
            Ok(Tensor::from_array((shape, vec))
                .map_err(|e| format!("Failed to build f16 tensor: {e}"))?
                .into())
        }
        _ => Ok(Tensor::from_array((shape, data.to_vec()))
            .map_err(|e| format!("Failed to build f32 tensor: {e}"))?
            .into()),
    }
}

fn build_label_tensor(
    shape: Vec<i64>,
    labels: &[i64],
    ty: TensorElementType,
) -> Result<SessionInputValue<'static>, String> {
    match ty {
        TensorElementType::Int32 => {
            let vec: Vec<i32> = labels.iter().map(|&v| v as i32).collect();
            Ok(Tensor::from_array((shape, vec))
                .map_err(|e| format!("Failed to build i32 label tensor: {e}"))?
                .into())
        }
        TensorElementType::Float32 => {
            let vec: Vec<f32> = labels.iter().map(|&v| v as f32).collect();
            Ok(Tensor::from_array((shape, vec))
                .map_err(|e| format!("Failed to build f32 label tensor: {e}"))?
                .into())
        }
        TensorElementType::Float16 => {
            let vec: Vec<f16> = labels.iter().map(|&v| f16::from_f32(v as f32)).collect();
            Ok(Tensor::from_array((shape, vec))
                .map_err(|e| format!("Failed to build f16 label tensor: {e}"))?
                .into())
        }
        _ => Ok(Tensor::from_array((shape, labels.to_vec()))
            .map_err(|e| format!("Failed to build i64 label tensor: {e}"))?
            .into()),
    }
}

fn build_cached_tensor_input(
    cached: &CachedTensor,
    expected_ty: TensorElementType,
) -> Result<SessionInputValue<'static>, String> {
    match (&cached.data, expected_ty) {
        (CachedTensorData::F16(v), TensorElementType::Float16) => {
            Ok(Tensor::from_array((cached.shape.clone(), v.clone()))
                .map_err(|e| format!("Failed to build f16 embedding tensor: {e}"))?
                .into())
        }
        (CachedTensorData::F32(v), TensorElementType::Float32) => {
            Ok(Tensor::from_array((cached.shape.clone(), v.clone()))
                .map_err(|e| format!("Failed to build f32 embedding tensor: {e}"))?
                .into())
        }
        (CachedTensorData::F16(v), TensorElementType::Float32) => {
            let conv: Vec<f32> = v.iter().map(|h| h.to_f32()).collect();
            Ok(Tensor::from_array((cached.shape.clone(), conv))
                .map_err(|e| format!("Failed to convert f16->f32 embedding tensor: {e}"))?
                .into())
        }
        (CachedTensorData::F32(v), TensorElementType::Float16) => {
            let conv: Vec<f16> = v.iter().map(|&x| f16::from_f32(x)).collect();
            Ok(Tensor::from_array((cached.shape.clone(), conv))
                .map_err(|e| format!("Failed to convert f32->f16 embedding tensor: {e}"))?
                .into())
        }
        (_, other) => Err(format!(
            "Unsupported embedding dtype {other} for '{}'",
            cached.name
        )),
    }
}

/// Preprocesses `LinearImage` → sRGB `[0, 1]` → bilinear resize to `spec.input_size × spec.input_size`
/// → `(c - mean) / std` in planar `[1, 3, H, W]` order.
pub(crate) fn preprocess_image_nchw_f32(image: &LinearImage, spec: &ModelSpec) -> Vec<f32> {
    let size = spec.input_size as usize;
    let plane = size * size;
    let mut out = vec![0.0_f32; 3 * plane];

    let sw = image.width as usize;
    let sh = image.height as usize;
    if sw == 0 || sh == 0 || image.rgba16f.len() < sw * sh * 4 {
        return out;
    }

    let mean = spec.norm_mean;
    let inv_std = [
        1.0 / spec.norm_std[0].max(1e-6),
        1.0 / spec.norm_std[1].max(1e-6),
        1.0 / spec.norm_std[2].max(1e-6),
    ];

    let sx_scale = sw as f32 / size as f32;
    let sy_scale = sh as f32 / size as f32;

    for dy in 0..size {
        let fy = ((dy as f32 + 0.5) * sy_scale - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        let row0 = y0 * sw;
        let row1 = y1 * sw;
        let dst_row = dy * size;

        for dx in 0..size {
            let fx = ((dx as f32 + 0.5) * sx_scale - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;

            let i00 = (row0 + x0) * 4;
            let i10 = (row0 + x1) * 4;
            let i01 = (row1 + x0) * 4;
            let i11 = (row1 + x1) * 4;

            let dst_idx = dst_row + dx;
            for ch in 0..3 {
                let p00 = image.rgba16f[i00 + ch].to_f32();
                let p10 = image.rgba16f[i10 + ch].to_f32();
                let p01 = image.rgba16f[i01 + ch].to_f32();
                let p11 = image.rgba16f[i11 + ch].to_f32();
                let top = p00 + (p10 - p00) * tx;
                let bot = p01 + (p11 - p01) * tx;
                let lin = top + (bot - top) * ty;
                let lin_clamped = if lin.is_finite() {
                    lin.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let srgb = linear_to_srgb(lin_clamped).clamp(0.0, 1.0);
                out[ch * plane + dst_idx] = (srgb - mean[ch]) * inv_std[ch];
            }
        }
    }

    out
}

fn resolve_prompt_for_sam(
    image: &LinearImage,
    prompt: &SmartMaskPrompt,
) -> (Vec<PromptPoint>, Option<PromptBox>, bool) {
    match prompt {
        SmartMaskPrompt::AutoSubject => (auto_seed_points(image), None, false),
        SmartMaskPrompt::AutoBackground => (auto_seed_points(image), None, true),
        SmartMaskPrompt::Interactive {
            points,
            bbox,
            invert,
        } => {
            if points.is_empty() && bbox.is_none() {
                (auto_seed_points(image), None, *invert)
            } else {
                (points.clone(), *bbox, *invert)
            }
        }
    }
}

fn auto_seed_points(image: &LinearImage) -> Vec<PromptPoint> {
    #[cfg(target_os = "macos")]
    let center = crate::vision_apple::foreground_centroid(image)
        .unwrap_or_else(|| crate::vision::estimate_saliency_centroid(image));

    #[cfg(not(target_os = "macos"))]
    let center = crate::vision::estimate_saliency_centroid(image);

    let mut pts = Vec::with_capacity(5);
    pts.push(PromptPoint::new(center[0], center[1], true));

    // Add 4 corner negative anchors when sufficiently far from the foreground centroid.
    for [cx, cy] in [[0.03_f32, 0.03], [0.97, 0.03], [0.03, 0.97], [0.97, 0.97]] {
        let dx = cx - center[0];
        let dy = cy - center[1];
        if dx * dx + dy * dy > 0.04 {
            pts.push(PromptPoint::new(cx, cy, false));
        }
    }
    pts
}

/// Downsamples `image` to a `dst_w × dst_h` perceptual CIE Lab guide grid for edge-aware
/// SAM logit de-checkerboarding.
fn downsample_image_to_lab_guide(image: &LinearImage, dst_w: usize, dst_h: usize) -> Vec<[f32; 3]> {
    let total = dst_w.saturating_mul(dst_h);
    let mut lab_grid = vec![[0.0_f32; 3]; total];
    let sw = image.width as usize;
    let sh = image.height as usize;
    if sw == 0 || sh == 0 || total == 0 || image.rgba16f.len() < sw * sh * 4 {
        return lab_grid;
    }

    let sx_scale = sw as f32 / dst_w as f32;
    let sy_scale = sh as f32 / dst_h as f32;

    for gy in 0..dst_h {
        let sy = (((gy as f32 + 0.5) * sy_scale).floor() as usize).min(sh - 1);
        let src_row = sy * sw;
        let dst_row = gy * dst_w;
        for gx in 0..dst_w {
            let sx = (((gx as f32 + 0.5) * sx_scale).floor() as usize).min(sw - 1);
            let base = (src_row + sx) * 4;
            let to_srgb = |h: f16| {
                let v = h.to_f32();
                if v.is_finite() {
                    linear_to_srgb(v.clamp(0.0, 1.0)).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            };
            let srgb = [
                to_srgb(image.rgba16f[base]),
                to_srgb(image.rgba16f[base + 1]),
                to_srgb(image.rgba16f[base + 2]),
            ];
            let (_, lab) = rgb_to_hsl_and_lab(srgb);
            lab_grid[dst_row + gx] = lab;
        }
    }
    lab_grid
}

/// Removes SAM 2.1 / SAM 3 `ConvTranspose2d` stride-2 and stride-4 checkerboard ripples from raw
/// decoder logits via a 2-pass separable image-guided joint bilateral filter.
///
/// The 5-tap spatial kernel `[0.5, 1.0, 1.0, 1.0, 0.5]` has exact mathematical zeros at both
/// `ω = π` (period 2) and `ω = π/2` (period 4), completely cancelling ViT/deconv grid artifacts
/// across smooth regions while preserving crisp object boundaries guided by the photo's CIE Lab
/// color edges.
fn refine_sam_logits_guided(logits: &[f32], w: usize, h: usize, image: &LinearImage) -> Vec<f32> {
    if w < 4 || h < 4 || logits.len() < w * h {
        return logits.to_vec();
    }

    let guide = downsample_image_to_lab_guide(image, w, h);
    // Exact period-2 and period-4 nulling symmetric kernel.
    const SPATIAL: [f32; 5] = [0.5, 1.0, 1.0, 1.0, 0.5];
    const INV_2_SIGMA_LAB2: f32 = 0.5 / (12.0 * 12.0);

    let mut cur = logits[..w * h].to_vec();
    let mut tmp = vec![0.0_f32; w * h];

    for _pass in 0..2 {
        // Horizontal pass
        for y in 0..h {
            let row = y * w;
            for x in 0..w {
                let c_lab = guide[row + x];
                let mut sum = 0.0_f32;
                let mut w_sum = 0.0_f32;
                for (ki, &sw) in SPATIAL.iter().enumerate() {
                    let nx = (x as isize + ki as isize - 2).clamp(0, (w - 1) as isize) as usize;
                    let n_lab = guide[row + nx];
                    let dl = n_lab[0] - c_lab[0];
                    let da = n_lab[1] - c_lab[1];
                    let db = n_lab[2] - c_lab[2];
                    let d2 = dl * dl + da * da + db * db;
                    let wt = sw * (-d2 * INV_2_SIGMA_LAB2).exp();
                    sum += cur[row + nx] * wt;
                    w_sum += wt;
                }
                tmp[row + x] = if w_sum > 1e-6 {
                    sum / w_sum
                } else {
                    cur[row + x]
                };
            }
        }

        // Vertical pass
        for y in 0..h {
            let row = y * w;
            for x in 0..w {
                let c_lab = guide[row + x];
                let mut sum = 0.0_f32;
                let mut w_sum = 0.0_f32;
                for (ki, &sw) in SPATIAL.iter().enumerate() {
                    let ny = (y as isize + ki as isize - 2).clamp(0, (h - 1) as isize) as usize;
                    let idx = ny * w + x;
                    let n_lab = guide[idx];
                    let dl = n_lab[0] - c_lab[0];
                    let da = n_lab[1] - c_lab[1];
                    let db = n_lab[2] - c_lab[2];
                    let d2 = dl * dl + da * da + db * db;
                    let wt = sw * (-d2 * INV_2_SIGMA_LAB2).exp();
                    sum += tmp[idx] * wt;
                    w_sum += wt;
                }
                cur[row + x] = if w_sum > 1e-6 {
                    sum / w_sum
                } else {
                    tmp[row + x]
                };
            }
        }
    }

    cur
}

/// Bilinearly upsamples raw refined SAM logits to the `512×512` `R8` mask tile BEFORE applying
/// a crisp anti-aliased sigmoid around `logit = 0.0` (matching official SAM 2/3 logit-space
/// interpolation while keeping pure background at `0` and solid interior at `255`).
fn upsample_logits_to_raster_mask(
    logits: &[f32],
    src_w: usize,
    src_h: usize,
    invert: bool,
) -> MaskShape {
    let grid = LAYER_MASK_TILE_SIZE;
    let mut bytes = vec![0u8; grid * grid];
    if src_w == 0 || src_h == 0 || logits.len() < src_w * src_h {
        if invert {
            bytes.fill(255);
        }
        return MaskShape::from_raster_u8(grid as u32, grid as u32, &bytes);
    }

    const LOGIT_GAIN: f32 = 4.5;
    let sx_scale = src_w as f32 / grid as f32;
    let sy_scale = src_h as f32 / grid as f32;
    let max_sx = (src_w - 1) as f32;
    let max_sy = (src_h - 1) as f32;

    for gy in 0..grid {
        let fy = ((gy as f32 + 0.5) * sy_scale - 0.5).clamp(0.0, max_sy);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let ty = fy - y0 as f32;
        let row0 = y0 * src_w;
        let row1 = y1 * src_w;
        let dst_row = gy * grid;

        for gx in 0..grid {
            let fx = ((gx as f32 + 0.5) * sx_scale - 0.5).clamp(0.0, max_sx);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let tx = fx - x0 as f32;

            let l00 = logits[row0 + x0];
            let l10 = logits[row0 + x1];
            let l01 = logits[row1 + x0];
            let l11 = logits[row1 + x1];
            let top = l00 + (l10 - l00) * tx;
            let bot = l01 + (l11 - l01) * tx;
            let logit = top + (bot - top) * ty;

            let scaled = (logit * LOGIT_GAIN).clamp(-20.0, 20.0);
            let mut prob = 1.0 / (1.0 + (-scaled).exp());
            if prob < 0.005 {
                prob = 0.0;
            } else if prob > 0.995 {
                prob = 1.0;
            }
            let q = (prob * 255.0).round() as u8;
            bytes[dst_row + gx] = if invert { 255 - q } else { q };
        }
    }

    MaskShape::from_raster_u8(grid as u32, grid as u32, &bytes)
}

/// Computes a fast deterministic 64-bit FNV-1a fingerprint of a `LinearImage` to key
/// the cached vision-encoder feature pyramid across repeated interactive clicks.
pub(crate) fn compute_image_fingerprint(image: &LinearImage) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    let mix = |h: &mut u64, bytes: &[u8]| {
        for &b in bytes {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(FNV_PRIME);
        }
    };

    mix(&mut hash, &image.width.to_le_bytes());
    mix(&mut hash, &image.height.to_le_bytes());
    mix(&mut hash, &(image.rgba16f.len() as u64).to_le_bytes());

    let px_count = (image.width as usize).saturating_mul(image.height as usize);
    if px_count > 0 && image.rgba16f.len() >= px_count * 4 {
        // Sample an evenly spaced 16×16 grid of pixels across the frame (256 samples).
        let stride = (px_count / 256).max(1);
        let mut idx = 0usize;
        while idx < px_count {
            let base = idx * 4;
            for ch in 0..3 {
                mix(&mut hash, &image.rgba16f[base + ch].to_bits().to_le_bytes());
            }
            idx += stride;
        }
        // Always include the very last pixel so edge edits invalidate the fingerprint.
        let last = (px_count - 1) * 4;
        for ch in 0..3 {
            mix(&mut hash, &image.rgba16f[last + ch].to_bits().to_le_bytes());
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_image(w: u32, h: u32, rgba16f: Vec<f16>) -> LinearImage {
        LinearImage {
            width: w,
            height: h,
            rgba16f,
            histogram_proxy: Vec::new(),
            display_name: "test".to_owned(),
            source_path: None,
            format_label: "Test".to_owned(),
            exif: None,
            camera_calibration: None,
        }
    }

    fn make_uniform_image(w: u32, h: u32, lin_rgb: [f32; 3]) -> LinearImage {
        let mut rgba16f = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            rgba16f.push(f16::from_f32(lin_rgb[0]));
            rgba16f.push(f16::from_f32(lin_rgb[1]));
            rgba16f.push(f16::from_f32(lin_rgb[2]));
            rgba16f.push(f16::from_f32(1.0));
        }
        make_test_image(w, h, rgba16f)
    }

    #[test]
    fn guided_logit_refinement_eliminates_stride4_checkerboard_ripples() {
        let size = 64usize;
        let image = make_uniform_image(size as u32, size as u32, [0.35, 0.25, 0.20]);

        // Synthetic SAM 2.1 ConvTranspose2d stride-4 grid: foreground around +1.8 with ±1.1
        // period-4 and period-2 ripples on the left half, background around -3.5 with ±0.9
        // ripples on the right half (no image edge on uniform test — verifies pure nulling).
        let mut fg_logits = vec![0.0_f32; size * size];
        let mut bg_logits = vec![0.0_f32; size * size];
        for y in 0..size {
            let cy4 = if y % 4 == 0 { 1.0 } else { -1.0 / 3.0 };
            let cy2 = if y % 2 == 0 { 0.4 } else { -0.4 };
            for x in 0..size {
                let cx4 = if x % 4 == 0 { 1.0 } else { -1.0 / 3.0 };
                let cx2 = if x % 2 == 0 { 0.4 } else { -0.4 };
                let ripple = 0.8 * (cx4 + cy4) + 0.5 * (cx2 + cy2);
                fg_logits[y * size + x] = 1.8 + ripple;
                bg_logits[y * size + x] = -3.5 + ripple;
            }
        }

        let clean_fg = refine_sam_logits_guided(&fg_logits, size, size, &image);
        let clean_bg = refine_sam_logits_guided(&bg_logits, size, size, &image);

        // Interior pixels must have virtually zero residual ripple.
        let mut fg_min = f32::INFINITY;
        let mut fg_max = f32::NEG_INFINITY;
        for y in 4..(size - 4) {
            for x in 4..(size - 4) {
                let v = clean_fg[y * size + x];
                fg_min = fg_min.min(v);
                fg_max = fg_max.max(v);
            }
        }
        assert!(
            (fg_max - fg_min) < 1e-4,
            "period-2/4 checkerboard ripple should be completely nulled, got range {fg_min}..{fg_max}"
        );

        let fg_mask = upsample_logits_to_raster_mask(&clean_fg, size, size, false);
        let bg_mask = upsample_logits_to_raster_mask(&clean_bg, size, size, false);
        let bg_inv = upsample_logits_to_raster_mask(&clean_bg, size, size, true);

        assert!(
            fg_mask
                .decode_raster_u8()
                .unwrap()
                .2
                .iter()
                .all(|&b| b == 255),
            "solid foreground logits should snap cleanly to 255"
        );
        assert!(
            bg_mask
                .decode_raster_u8()
                .unwrap()
                .2
                .iter()
                .all(|&b| b == 0),
            "background logits should snap cleanly to 0 with no leak"
        );
        assert!(
            bg_inv
                .decode_raster_u8()
                .unwrap()
                .2
                .iter()
                .all(|&b| b == 255),
            "inverted background logits should snap cleanly to 255"
        );
    }

    #[test]
    fn guided_logit_refinement_preserves_sharp_photo_edges() {
        let size = 64usize;
        let mut rgba16f = Vec::with_capacity(size * size * 4);
        let mut logits = vec![0.0_f32; size * size];
        for y in 0..size {
            for x in 0..size {
                // High-contrast vertical boundary at x = 32: dark foreground on left, bright bg on right.
                let is_fg = x < 32;
                let lum = if is_fg { 0.04 } else { 0.82 };
                rgba16f.push(f16::from_f32(lum));
                rgba16f.push(f16::from_f32(lum));
                rgba16f.push(f16::from_f32(lum));
                rgba16f.push(f16::from_f32(1.0));

                let ripple = if (x % 4 == 0) ^ (y % 4 == 0) {
                    0.9
                } else {
                    -0.9
                };
                logits[y * size + x] = if is_fg { 2.5 + ripple } else { -3.0 + ripple };
            }
        }
        let image = make_test_image(size as u32, size as u32, rgba16f);

        let clean = refine_sam_logits_guided(&logits, size, size, &image);
        // Right next to the boundary (x = 31 vs x = 32), the Lab edge must stop logit bleed.
        let left_edge = clean[32 * size + 31];
        let right_edge = clean[32 * size + 32];
        assert!(
            left_edge > 1.5,
            "foreground edge pixel should remain strongly positive, got {left_edge}"
        );
        assert!(
            right_edge < -1.5,
            "background edge pixel should remain strongly negative, got {right_edge}"
        );
    }
}
