//! Offline, deterministic AI Subject & Background Mask generation (`256×256` R8 base64).
//!
//! Uses a GrabCut-inspired multi-prototype CIE Lab border-background vs center-prior foreground
//! saliency model refined with two passes of joint-bilateral edge-guided smoothing so mask
//! silhouettes snap cleanly onto real photographic subject boundaries with zero external crates
//! and zero crash risk in headless or cross-platform environments.

use rune_editor_core::{LAYER_MASK_TILE_SIZE, MaskShape, rgb_to_hsl_and_lab};

use crate::LinearImage;

/// Generates a crisp, edge-guided `256×256` `MaskShape::RasterMask` isolating the primary
/// foreground subject (`background = false`) or its inverse background (`background = true`).
pub fn generate_subject_mask(image: &LinearImage, background: bool) -> MaskShape {
    let grid = LAYER_MASK_TILE_SIZE;
    let total = grid * grid;
    if image.width == 0 || image.height == 0 || image.rgba16f.len() < 4 {
        let fill = if background { 255u8 } else { 0u8 };
        return MaskShape::from_raster_u8(grid as u32, grid as u32, &vec![fill; total]);
    }

    let sw = image.width as usize;
    let sh = image.height as usize;

    // 1. Downsample source linear image into 256×256 perceptual CIE Lab grid.
    let mut lab_grid = vec![[0.0_f32; 3]; total];
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

    // 2. Build 4 directional border background prototypes (top, bottom, left, right strips)
    //    so split backgrounds (e.g. bright sky + dark ground) are represented accurately.
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

    // 3. Estimate foreground prototype from center region weighted by Lab distance from border.
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

    // Check if the image is nearly uniform (flat frame): fall back gracefully to a soft center oval.
    let uniform_scene = max_bg_dist < 4.0;

    // 4. Compute raw foreground probability combining Lab color contrast + soft center prior.
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
                // Suppress extreme frame borders softly so backdrop corners stay clean.
                let edge_dist = u.min(1.0 - u).min(v).min(1.0 - v);
                let border_fade = smoothstep(0.0, 0.06, edge_dist);
                let raw = (color_ratio * 0.62 + bg_contrast * 0.23 + center_prior * 0.15)
                    * (0.35 + 0.65 * border_fade);
                // Crisp S-curve around foreground threshold while preserving soft edge gradients.
                smoothstep(0.30, 0.68, raw)
            };
            prob[row + gx] = p;
        }
    }

    // 5. Two fast edge-preserving joint-bilateral refinement passes guided by Lab image.
    let mut scratch = vec![0.0_f32; total];
    for _ in 0..2 {
        joint_bilateral_pass(&prob, &lab_grid, grid, &mut scratch);
        std::mem::swap(&mut prob, &mut scratch);
    }

    // 6. Quantize to R8 (inverting cleanly when `background = true` so subject + bg == 255).
    let mut bytes = vec![0u8; total];
    for (dst, &p) in bytes.iter_mut().zip(prob.iter()) {
        let q = (p.clamp(0.0, 1.0) * 255.0).round() as u8;
        *dst = if background { 255 - q } else { q };
    }

    MaskShape::from_raster_u8(grid as u32, grid as u32, &bytes)
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
