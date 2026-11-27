//! macOS Apple Vision Neural Engine (ANE) subject & interactive instance segmentation backend.
//!
//! Uses `VNGenerateForegroundInstanceMaskRequest` on macOS 14+ (zero-download, built-in ANE)
//! for both automatic subject/background matting and interactive instance selection via
//! `instanceMask` + `generateScaledMaskForImageForInstances:fromRequestHandler:error:`.
//! Falls back cleanly to `VNGeneratePersonSegmentationRequest` (Accurate quality) on macOS 12–13
//! using runtime class & selector checks so older macOS versions never crash.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::ptr;

use half::f16;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{AllocAnyThread, sel};
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    kCGColorSpaceSRGB,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_OneComponent8, kCVPixelFormatType_OneComponent16Half,
    kCVPixelFormatType_OneComponent32Float, kCVReturnSuccess,
};
use objc2_foundation::{
    NSArray, NSCopying, NSDictionary, NSIndexSet, NSMutableIndexSet, NSObjectProtocol,
};
use objc2_vision::{
    VNGenerateForegroundInstanceMaskRequest, VNGeneratePersonSegmentationRequest,
    VNGeneratePersonSegmentationRequestQualityLevel, VNImageOption, VNImageRequestHandler,
    VNInstanceMaskObservation, VNRequest,
};
use rune_editor_core::{
    LAYER_MASK_TILE_SIZE, MaskShape, PromptBox, PromptPoint, SmartMaskPrompt, linear_to_srgb,
};

use crate::LinearImage;
use crate::raw::fit_dimensions;

/// Maximum long-edge resolution sent to Apple Vision so inference stays fast (<150ms) and crisp.
const VISION_MAX_LONG_EDGE: u32 = 1536;

/// Structured result returned by the macOS Apple Vision backend.
#[derive(Clone, Debug)]
pub struct VisionAppleResult {
    pub mask: MaskShape,
    pub instance_count: usize,
    pub backend_label: &'static str,
}

/// Returns `true` when Apple Vision foreground instance segmentation (macOS 14+) or
/// person segmentation (macOS 12+) is available at runtime.
pub fn is_available() -> bool {
    has_foreground_instance_api() || has_person_segmentation_api()
}

/// Returns `true` when macOS 14+ foreground instance segmentation (required for interactive
/// point/box instance picking) is available at runtime.
pub fn supports_interactive() -> bool {
    has_foreground_instance_api()
}

/// Human-readable label of the strongest Apple Vision API available on this macOS host.
pub fn available_backend_label() -> &'static str {
    if has_foreground_instance_api() {
        "Apple Vision ANE (macOS 14+ Foreground Instance)"
    } else if has_person_segmentation_api() {
        "Apple Vision ANE (macOS 12+ Person Segmentation)"
    } else {
        "Apple Vision (Unavailable)"
    }
}

fn has_foreground_instance_api() -> bool {
    AnyClass::get(c"VNGenerateForegroundInstanceMaskRequest").is_some()
}

fn has_person_segmentation_api() -> bool {
    AnyClass::get(c"VNGeneratePersonSegmentationRequest").is_some()
}

/// RAII guard guaranteeing `CVPixelBufferUnlockBaseAddress` runs on every exit path.
struct PixelBufferReadLock<'a> {
    buffer: &'a CVPixelBuffer,
}

impl<'a> PixelBufferReadLock<'a> {
    fn lock(buffer: &'a CVPixelBuffer) -> Result<Self, String> {
        let status =
            unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        if status != kCVReturnSuccess {
            return Err(format!(
                "CVPixelBufferLockBaseAddress failed with status {status}"
            ));
        }
        Ok(Self { buffer })
    }
}

impl Drop for PixelBufferReadLock<'_> {
    fn drop(&mut self) {
        let _ = unsafe {
            CVPixelBufferUnlockBaseAddress(self.buffer, CVPixelBufferLockFlags::ReadOnly)
        };
    }
}

/// Runs Apple Vision segmentation on `image` for the requested `prompt`.
pub fn segment(image: &LinearImage, prompt: &SmartMaskPrompt) -> Result<VisionAppleResult, String> {
    if !image.has_valid_buffer() {
        return Err("Cannot run Apple Vision on an empty or truncated image buffer".to_owned());
    }

    let prompt = prompt.sanitized();

    autoreleasepool(|_| {
        let cg_image = linear_image_to_srgb_cgimage(image, VISION_MAX_LONG_EDGE)?;
        let empty_opts = NSDictionary::<VNImageOption, AnyObject>::new();
        let handler = unsafe {
            VNImageRequestHandler::initWithCGImage_options(
                VNImageRequestHandler::alloc(),
                &cg_image,
                &empty_opts,
            )
        };

        if has_foreground_instance_api() {
            match run_foreground_instance_mask(&handler, &prompt) {
                Ok(res) => return Ok(res),
                Err(fg_err) => {
                    // For AutoSubject/AutoBackground, try PersonSegmentation fallback if
                    // ForegroundInstanceMask found no instances or failed on an edge case.
                    if matches!(
                        prompt,
                        SmartMaskPrompt::AutoSubject | SmartMaskPrompt::AutoBackground
                    ) && has_person_segmentation_api()
                        && let Ok(res) = run_person_segmentation(&handler, &prompt)
                    {
                        return Ok(res);
                    }
                    return Err(fg_err);
                }
            }
        }

        // On macOS 12–13 (PersonSegmentation only), do not silently ignore interactive
        // points/boxes — return an error so `Auto` cleanly falls through to interactive fallback.
        if let SmartMaskPrompt::Interactive { points, bbox, .. } = &prompt
            && (!points.is_empty() || bbox.is_some())
        {
            return Err(
                "Apple Vision interactive instance selection requires macOS 14 or later".to_owned(),
            );
        }

        if has_person_segmentation_api() {
            return run_person_segmentation(&handler, &prompt);
        }

        Err("Apple Vision segmentation requires macOS 12 or later".to_owned())
    })
}

/// Computes the normalized `[0.0, 1.0]` centroid of Apple Vision's detected foreground instances
/// (used by SAM ONNX backends to seed automatic subject segmentation accurately on macOS).
pub(crate) fn foreground_centroid(image: &LinearImage) -> Option<[f32; 2]> {
    if !has_foreground_instance_api() || !image.has_valid_buffer() {
        return None;
    }

    autoreleasepool(|_| {
        let cg_image = linear_image_to_srgb_cgimage(image, 512).ok()?;
        let empty_opts = NSDictionary::<VNImageOption, AnyObject>::new();
        let handler = unsafe {
            VNImageRequestHandler::initWithCGImage_options(
                VNImageRequestHandler::alloc(),
                &cg_image,
                &empty_opts,
            )
        };

        let request = unsafe { VNGenerateForegroundInstanceMaskRequest::new() };
        let req_ref: &VNRequest = &request;
        let requests = NSArray::from_slice(&[req_ref]);
        handler.performRequests_error(&requests).ok()?;

        let results = unsafe { request.results() }?;
        if results.is_empty() {
            return None;
        }
        let observation = results.objectAtIndex(0);
        let instance_buf = unsafe { observation.instanceMask() };
        let (bw, bh, ids) = read_instance_id_buffer(&instance_buf).ok()?;
        if bw == 0 || bh == 0 {
            return None;
        }

        let mut sum_x = 0.0_f64;
        let mut sum_y = 0.0_f64;
        let mut count = 0u64;
        for y in 0..bh {
            let row = y * bw;
            for x in 0..bw {
                if ids[row + x] != 0 {
                    sum_x += (x as f64 + 0.5) / bw as f64;
                    sum_y += (y as f64 + 0.5) / bh as f64;
                    count += 1;
                }
            }
        }
        if count == 0 {
            return None;
        }
        Some([
            (sum_x / count as f64).clamp(0.02, 0.98) as f32,
            (sum_y / count as f64).clamp(0.02, 0.98) as f32,
        ])
    })
}

fn run_foreground_instance_mask(
    handler: &VNImageRequestHandler,
    prompt: &SmartMaskPrompt,
) -> Result<VisionAppleResult, String> {
    let request = unsafe { VNGenerateForegroundInstanceMaskRequest::new() };
    let req_ref: &VNRequest = &request;
    let requests = NSArray::from_slice(&[req_ref]);

    handler
        .performRequests_error(&requests)
        .map_err(|err| format!("VNGenerateForegroundInstanceMaskRequest failed: {err}"))?;

    let results = unsafe { request.results() }
        .ok_or_else(|| "VNGenerateForegroundInstanceMaskRequest returned nil results".to_owned())?;
    if results.is_empty() {
        return Err("Apple Vision detected no foreground instances".to_owned());
    }

    let observation: Retained<VNInstanceMaskObservation> = results.objectAtIndex(0);
    let all_instances = unsafe { observation.allInstances() };
    let instance_count = all_instances.count();
    if instance_count == 0 {
        return Err("Apple Vision returned an empty foreground instance set".to_owned());
    }

    let (selected_instances, invert) = match prompt {
        SmartMaskPrompt::AutoSubject => (all_instances, false),
        SmartMaskPrompt::AutoBackground => (all_instances, true),
        SmartMaskPrompt::Interactive {
            points,
            bbox,
            invert,
        } => {
            let selected = select_instances_for_interactive(
                &observation,
                &all_instances,
                points,
                bbox.as_ref(),
            )?;
            (selected, *invert)
        }
    };

    let mask_buf = if observation
        .respondsToSelector(sel!(generateScaledMaskForImageForInstances:fromRequestHandler:error:))
    {
        unsafe {
            observation.generateScaledMaskForImageForInstances_fromRequestHandler_error(
                &selected_instances,
                handler,
            )
        }
        .map_err(|err| {
            format!(
                "generateScaledMaskForImageForInstances:fromRequestHandler:error: failed: {err}"
            )
        })?
    } else {
        unsafe { observation.generateMaskForInstances_error(&selected_instances) }
            .map_err(|err| format!("generateMaskForInstances:error: failed: {err}"))?
    };

    let (mw, mh, alpha_f32) = read_alpha_buffer(&mask_buf)?;
    let mask = resample_alpha_to_raster_mask(&alpha_f32, mw, mh, invert);

    Ok(VisionAppleResult {
        mask,
        instance_count,
        backend_label: "Apple Vision ANE (Foreground Instance)",
    })
}

fn select_instances_for_interactive(
    observation: &VNInstanceMaskObservation,
    all_instances: &NSIndexSet,
    points: &[PromptPoint],
    bbox: Option<&PromptBox>,
) -> Result<Retained<NSIndexSet>, String> {
    if points.is_empty() && bbox.is_none() {
        return Ok(all_instances.copy());
    }

    let instance_buf = unsafe { observation.instanceMask() };
    let (bw, bh, ids) = read_instance_id_buffer(&instance_buf)?;
    if bw == 0 || bh == 0 {
        return Ok(all_instances.copy());
    }

    let mut positive_ids: HashSet<usize> = HashSet::new();
    let mut negative_ids: HashSet<usize> = HashSet::new();
    let search_radius = (bw.min(bh) / 20).clamp(4, 32) as isize;

    for pt in points {
        let px = ((pt.x.clamp(0.0, 1.0) * (bw - 1) as f32).round() as usize).min(bw - 1);
        let py = ((pt.y.clamp(0.0, 1.0) * (bh - 1) as f32).round() as usize).min(bh - 1);
        if pt.positive {
            if let Some(id) = sample_instance_id_with_radius(&ids, bw, bh, px, py, search_radius)
                && all_instances.containsIndex(id)
            {
                positive_ids.insert(id);
            }
        } else if let Some(id) = sample_instance_id_with_radius(&ids, bw, bh, px, py, 2)
            && all_instances.containsIndex(id)
        {
            negative_ids.insert(id);
        }
    }

    if let Some(b) = bbox {
        let x0 = ((b.x0.clamp(0.0, 1.0) * bw as f32).floor() as usize).min(bw - 1);
        let x1 = ((b.x1.clamp(0.0, 1.0) * bw as f32).ceil() as usize).clamp(x0 + 1, bw);
        let y0 = ((b.y0.clamp(0.0, 1.0) * bh as f32).floor() as usize).min(bh - 1);
        let y1 = ((b.y1.clamp(0.0, 1.0) * bh as f32).ceil() as usize).clamp(y0 + 1, bh);

        let mut counts: HashMap<usize, usize> = HashMap::new();
        for y in y0..y1 {
            let row = y * bw;
            for x in x0..x1 {
                let id = ids[row + x] as usize;
                if id != 0 && all_instances.containsIndex(id) {
                    *counts.entry(id).or_insert(0) += 1;
                }
            }
        }
        if !counts.is_empty() {
            let max_count = counts.values().copied().max().unwrap_or(0);
            let threshold = (max_count / 5).max(1);
            for (id, cnt) in counts {
                if cnt >= threshold {
                    positive_ids.insert(id);
                }
            }
        }
    }

    let has_positive_prompts = points.iter().any(|p| p.positive) || bbox.is_some();

    let chosen: HashSet<usize> = if !positive_ids.is_empty() {
        let filtered: HashSet<usize> = positive_ids
            .iter()
            .copied()
            .filter(|id| !negative_ids.contains(id))
            .collect();
        if filtered.is_empty() {
            // Keep positive selection when negative point landed on the same instance.
            positive_ids
        } else {
            filtered
        }
    } else if !has_positive_prompts && !negative_ids.is_empty() {
        // Negative-only prompt: keep all foreground instances except the excluded ones.
        let mut remaining = HashSet::new();
        let mut idx = all_instances.firstIndex();
        while idx != usize::MAX && idx != objc2_foundation::NSNotFound as usize {
            if !negative_ids.contains(&idx) {
                remaining.insert(idx);
            }
            idx = all_instances.indexGreaterThanIndex(idx);
        }
        remaining
    } else {
        HashSet::new()
    };

    if chosen.is_empty() {
        return Ok(all_instances.copy());
    }

    let mutable = NSMutableIndexSet::indexSet();
    for id in chosen {
        mutable.addIndex(id);
    }
    Ok(mutable.into_super())
}

fn sample_instance_id_with_radius(
    ids: &[u8],
    bw: usize,
    bh: usize,
    cx: usize,
    cy: usize,
    max_radius: isize,
) -> Option<usize> {
    let exact = ids[cy * bw + cx] as usize;
    if exact != 0 {
        return Some(exact);
    }
    for r in 1..=max_radius {
        let mut best_id = 0usize;
        let mut best_d2 = i64::MAX;
        for dy in -r..=r {
            let ny = cy as isize + dy;
            if ny < 0 || ny >= bh as isize {
                continue;
            }
            let row = (ny as usize) * bw;
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue;
                }
                let nx = cx as isize + dx;
                if nx < 0 || nx >= bw as isize {
                    continue;
                }
                let id = ids[row + nx as usize] as usize;
                if id != 0 {
                    let d2 = (dx * dx + dy * dy) as i64;
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best_id = id;
                    }
                }
            }
        }
        if best_id != 0 {
            return Some(best_id);
        }
    }
    None
}

fn run_person_segmentation(
    handler: &VNImageRequestHandler,
    prompt: &SmartMaskPrompt,
) -> Result<VisionAppleResult, String> {
    let request = unsafe { VNGeneratePersonSegmentationRequest::new() };
    unsafe {
        request.setQualityLevel(VNGeneratePersonSegmentationRequestQualityLevel::Accurate);
        request.setOutputPixelFormat(kCVPixelFormatType_OneComponent8);
    }
    let req_ref: &VNRequest = &request;
    let requests = NSArray::from_slice(&[req_ref]);

    handler
        .performRequests_error(&requests)
        .map_err(|err| format!("VNGeneratePersonSegmentationRequest failed: {err}"))?;

    let results = unsafe { request.results() }
        .ok_or_else(|| "VNGeneratePersonSegmentationRequest returned nil results".to_owned())?;
    if results.is_empty() {
        return Err("Apple Vision person segmentation returned no observations".to_owned());
    }

    let observation = results.objectAtIndex(0);
    let mask_buf = unsafe { observation.pixelBuffer() };
    let (mw, mh, alpha_f32) = read_alpha_buffer(&mask_buf)?;

    let invert = match prompt {
        SmartMaskPrompt::AutoSubject => false,
        SmartMaskPrompt::AutoBackground => true,
        SmartMaskPrompt::Interactive { invert, .. } => *invert,
    };

    let mask = resample_alpha_to_raster_mask(&alpha_f32, mw, mh, invert);
    Ok(VisionAppleResult {
        mask,
        instance_count: 1,
        backend_label: "Apple Vision ANE (Person Segmentation)",
    })
}

/// Converts a scene-referred linear `LinearImage` to an 8-bit sRGB `CGImage`, downscaling
/// cleanly to `max_long_edge` while preserving exact aspect ratio.
fn linear_image_to_srgb_cgimage(
    image: &LinearImage,
    max_long_edge: u32,
) -> Result<CFRetained<CGImage>, String> {
    let (dst_w, dst_h) = fit_dimensions(image.width, image.height, max_long_edge);
    let sw = image.width as usize;
    let sh = image.height as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;

    let mut rgba8 = vec![0u8; dw * dh * 4];
    let sx_scale = sw as f32 / dw as f32;
    let sy_scale = sh as f32 / dh as f32;

    for dy in 0..dh {
        let fy = ((dy as f32 + 0.5) * sy_scale - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        let row0 = y0 * sw;
        let row1 = y1 * sw;
        let dst_row = dy * dw * 4;

        for dx in 0..dw {
            let fx = ((dx as f32 + 0.5) * sx_scale - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;

            let i00 = (row0 + x0) * 4;
            let i10 = (row0 + x1) * 4;
            let i01 = (row1 + x0) * 4;
            let i11 = (row1 + x1) * 4;

            let out_base = dst_row + dx * 4;
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
                let srgb = linear_to_srgb(lin_clamped);
                rgba8[out_base + ch] = (srgb.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            rgba8[out_base + 3] = 255;
        }
    }

    let cf_data = CFData::from_bytes(&rgba8);
    let provider = CGDataProvider::with_cf_data(Some(&cf_data))
        .ok_or_else(|| "CGDataProvider::with_cf_data returned null".to_owned())?;
    let color_space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .ok_or_else(|| "CGColorSpace::with_name(kCGColorSpaceSRGB) returned null".to_owned())?;
    let bitmap_info = CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0);

    unsafe {
        CGImage::new(
            dw,
            dh,
            8,
            32,
            dw * 4,
            Some(&color_space),
            bitmap_info,
            Some(&provider),
            ptr::null(),
            true,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(|| "CGImage::new returned null".to_owned())
}

fn read_instance_id_buffer(buf: &CVPixelBuffer) -> Result<(usize, usize, Vec<u8>), String> {
    let _lock = PixelBufferReadLock::lock(buf)?;
    let w = CVPixelBufferGetWidth(buf);
    let h = CVPixelBufferGetHeight(buf);
    let bpr = CVPixelBufferGetBytesPerRow(buf);
    let fmt = CVPixelBufferGetPixelFormatType(buf);
    let base = CVPixelBufferGetBaseAddress(buf) as *const u8;
    if base.is_null() || w == 0 || h == 0 {
        return Err("CVPixelBuffer instanceMask base address is null or empty".to_owned());
    }
    if fmt != kCVPixelFormatType_OneComponent8 {
        return Err(format!(
            "Unexpected instanceMask pixel format 0x{fmt:08x} (expected OneComponent8)"
        ));
    }
    if bpr < w {
        return Err(format!(
            "CVPixelBuffer bytesPerRow ({bpr}) smaller than width ({w})"
        ));
    }

    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let src_row = unsafe { std::slice::from_raw_parts(base.add(y * bpr), w) };
        out[y * w..(y + 1) * w].copy_from_slice(src_row);
    }
    Ok((w, h, out))
}

fn read_alpha_buffer(buf: &CVPixelBuffer) -> Result<(usize, usize, Vec<f32>), String> {
    let _lock = PixelBufferReadLock::lock(buf)?;
    let w = CVPixelBufferGetWidth(buf);
    let h = CVPixelBufferGetHeight(buf);
    let bpr = CVPixelBufferGetBytesPerRow(buf);
    let fmt = CVPixelBufferGetPixelFormatType(buf);
    let base: *const c_void = CVPixelBufferGetBaseAddress(buf);
    if base.is_null() || w == 0 || h == 0 {
        return Err("CVPixelBuffer mask base address is null or empty".to_owned());
    }
    let base_bytes = base.cast::<u8>();
    let mut out = vec![0.0_f32; w * h];

    match fmt {
        x if x == kCVPixelFormatType_OneComponent32Float => {
            if bpr < w * 4 {
                return Err(format!(
                    "OneComponent32Float bytesPerRow ({bpr}) < width*4 ({})",
                    w * 4
                ));
            }
            for y in 0..h {
                let row_ptr = unsafe { base_bytes.add(y * bpr) };
                let dst_row = &mut out[y * w..(y + 1) * w];
                for (x, dst) in dst_row.iter_mut().enumerate() {
                    let val = unsafe { ptr::read_unaligned(row_ptr.add(x * 4).cast::<f32>()) };
                    *dst = if val.is_finite() {
                        val.clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                }
            }
        }
        x if x == kCVPixelFormatType_OneComponent16Half => {
            if bpr < w * 2 {
                return Err(format!(
                    "OneComponent16Half bytesPerRow ({bpr}) < width*2 ({})",
                    w * 2
                ));
            }
            for y in 0..h {
                let row_ptr = unsafe { base_bytes.add(y * bpr) };
                let dst_row = &mut out[y * w..(y + 1) * w];
                for (x, dst) in dst_row.iter_mut().enumerate() {
                    let bits = unsafe { ptr::read_unaligned(row_ptr.add(x * 2).cast::<u16>()) };
                    let val = f16::from_bits(bits).to_f32();
                    *dst = if val.is_finite() {
                        val.clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                }
            }
        }
        x if x == kCVPixelFormatType_OneComponent8 => {
            if bpr < w {
                return Err(format!("OneComponent8 bytesPerRow ({bpr}) < width ({w})"));
            }
            for y in 0..h {
                let src_row = unsafe { std::slice::from_raw_parts(base_bytes.add(y * bpr), w) };
                let dst_row = &mut out[y * w..(y + 1) * w];
                for (dst, &byte) in dst_row.iter_mut().zip(src_row.iter()) {
                    *dst = byte as f32 * (1.0 / 255.0);
                }
            }
        }
        other => {
            return Err(format!(
                "Unsupported CVPixelBuffer mask pixel format 0x{other:08x}"
            ));
        }
    }

    Ok((w, h, out))
}

/// Bilinearly resamples a `[0.0, 1.0]` alpha buffer into a `LAYER_MASK_TILE_SIZE × LAYER_MASK_TILE_SIZE`
/// (`512×512`) R8 `MaskShape::RasterMask`.
pub(crate) fn resample_alpha_to_raster_mask(
    alpha: &[f32],
    src_w: usize,
    src_h: usize,
    invert: bool,
) -> MaskShape {
    let grid = LAYER_MASK_TILE_SIZE;
    let mut bytes = vec![0u8; grid * grid];

    if src_w == 0 || src_h == 0 || alpha.len() < src_w * src_h {
        if invert {
            bytes.fill(255);
        }
        return MaskShape::from_raster_u8(grid as u32, grid as u32, &bytes);
    }

    let max_sx = (src_w - 1) as f32;
    let max_sy = (src_h - 1) as f32;
    let scale_x = if grid > 1 {
        max_sx / (grid - 1) as f32
    } else {
        0.0
    };
    let scale_y = if grid > 1 {
        max_sy / (grid - 1) as f32
    } else {
        0.0
    };

    for gy in 0..grid {
        let fy = (gy as f32 * scale_y).clamp(0.0, max_sy);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let ty = fy - y0 as f32;
        let row0 = y0 * src_w;
        let row1 = y1 * src_w;
        let dst_row = gy * grid;

        for gx in 0..grid {
            let fx = (gx as f32 * scale_x).clamp(0.0, max_sx);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let tx = fx - x0 as f32;

            let p00 = alpha[row0 + x0];
            let p10 = alpha[row0 + x1];
            let p01 = alpha[row1 + x0];
            let p11 = alpha[row1 + x1];
            let top = p00 + (p10 - p00) * tx;
            let bot = p01 + (p11 - p01) * tx;
            let v = (top + (bot - top) * ty).clamp(0.0, 1.0);
            let q = (v * 255.0).round() as u8;
            bytes[dst_row + gx] = if invert { 255 - q } else { q };
        }
    }

    MaskShape::from_raster_u8(grid as u32, grid as u32, &bytes)
}
