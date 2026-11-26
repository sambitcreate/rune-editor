//! Quantitative perceptual image analysis (`analyze`) and deterministic CIEDE2000
//! recipe comparison (`diff`) for AI vision agents.

use serde::{Deserialize, Serialize};

use crate::{
    DevelopRecipe, HSL_BAND_HUES, HSL_BAND_NAMES, auto_exposure_and_tone, auto_levels,
    auto_white_balance, filmic_tonemap_with_curve, linear_to_srgb, patch::FieldDiff,
    registry::recipe_field_diffs, rgb_to_hsl_and_lab,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LumaPercentiles {
    pub p01: f32,
    pub p05: f32,
    pub p25: f32,
    pub p50: f32,
    pub p75: f32,
    pub p95: f32,
    pub p99: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvPercentiles {
    pub p05: f32,
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipMetrics {
    pub shadow_all: f32,
    pub highlight_any: f32,
    pub highlight_all: f32,
    pub per_channel_shadow: [f32; 3],
    pub per_channel_highlight: [f32; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneAnalysis {
    pub luma_srgb_percentiles: LumaPercentiles,
    pub luma_ev_rel_18pct: EvPercentiles,
    pub dynamic_range_stops_p01_p99: f32,
    pub key: String,
    pub hdr_headroom_pct: f32,
    pub clip_pct: ClipMetrics,
    pub zones_0_to_10_pct: [f32; 11],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NeutralCast {
    pub a_star: f32,
    pub b_star: f32,
    pub neutral_sample_pct: f32,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TonalCast {
    pub shadows: [f32; 2],
    pub midtones: [f32; 2],
    pub highlights: [f32; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkinToneMetrics {
    pub coverage_pct: f32,
    pub mean_lab: [f32; 3],
    pub hue_deg: f32,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaletteSwatch {
    pub hex: String,
    pub pct: f32,
    pub hsl: [f32; 3],
    pub lab: [f32; 3],
    pub band: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorAnalysis {
    pub cast_lab_neutral: NeutralCast,
    pub cast_by_tone_ab: TonalCast,
    pub chroma_mean: f32,
    pub chroma_p95: f32,
    pub saturation_mean_pct: f32,
    pub saturation_p95_pct: f32,
    pub skin_tones: SkinToneMetrics,
    pub palette: Vec<PaletteSwatch>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusAnalysis {
    pub sharpness_score: f32,
    pub laplacian_var: f32,
    pub tenengrad: f32,
    pub focus_mask_pct: f32,
    pub flat_noise_est: f32,
    pub sharpness_zones_3x3: [[f32; 3]; 3],
    pub luma_zones_3x3: [[f32; 3]; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoSuggestions {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub temperature: f32,
    pub tint: f32,
    pub levels_in_black: f32,
    pub levels_in_mid: f32,
    pub levels_in_white: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageAnalysis {
    pub tone: ToneAnalysis,
    pub color: ColorAnalysis,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<FocusAnalysis>,
    pub suggested_auto: AutoSuggestions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaEStats {
    pub mean: f32,
    pub p95: f32,
    pub max: f32,
    pub shadows: f32,
    pub midtones: f32,
    pub highlights: f32,
    pub skin_tones: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LumaShift {
    pub mean_delta_ev: f32,
    pub p50_delta_lstar: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipDelta {
    pub shadow_all: f32,
    pub highlight_any: f32,
    pub recovered_highlights: f32,
    pub newly_clipped_highlights: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorShift {
    pub delta_a_star_neutral: f32,
    pub delta_b_star_neutral: f32,
    pub chroma_ratio: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeDiff {
    pub delta_e2000: DeltaEStats,
    pub luma_shift: LumaShift,
    pub clip_delta_pct: ClipDelta,
    pub color_shift: ColorShift,
    pub recipe_changes: Vec<FieldDiff>,
}

#[inline]
fn luma709(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[inline]
fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

#[inline]
fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

fn percentile_sorted(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f32 * q.clamp(0.0, 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Evaluates quantitative tonal, color, skin, palette, and optional sharpness metrics
/// for a linear RGB proxy under `recipe`.
pub fn analyze_linear_proxy(
    raw_proxy: &[[f32; 3]],
    recipe: &DevelopRecipe,
    detail_luma: Option<(&[f32], u32, u32)>,
) -> ImageAnalysis {
    let compiled = recipe.compile();
    let n = raw_proxy.len().max(1);
    let nf = n as f32;

    let mut luma_srgb = Vec::with_capacity(n);
    let mut luma_ev = Vec::with_capacity(n);
    let mut chromas = Vec::with_capacity(n);
    let mut sats = Vec::with_capacity(n);
    let mut labs = Vec::with_capacity(n);

    let mut hdr_headroom = 0usize;
    let mut shadow_all = 0usize;
    let mut hi_any = 0usize;
    let mut hi_all = 0usize;
    let mut ch_sh = [0usize; 3];
    let mut ch_hi = [0usize; 3];
    let mut zones = [0usize; 11];

    let mut neu_a = 0.0_f64;
    let mut neu_b = 0.0_f64;
    let mut neu_n = 0usize;
    let mut all_a = 0.0_f64;
    let mut all_b = 0.0_f64;

    let mut tone_ab = [[0.0_f64; 2]; 3];
    let mut tone_n = [0usize; 3];

    let mut skin_lab = [0.0_f64; 3];
    let mut skin_n = 0usize;

    for &raw_px in raw_proxy {
        let scene = compiled.evaluate_pixel_scene_linear(raw_px);
        if scene[0] > 1.0 || scene[1] > 1.0 || scene[2] > 1.0 {
            hdr_headroom += 1;
        }
        let ev = (luma709(scene).max(1e-5) / 0.18).log2();
        luma_ev.push(ev);

        let disp = filmic_tonemap_with_curve(scene, compiled.film_curve);
        let srgb = [
            linear_to_srgb(disp[0]),
            linear_to_srgb(disp[1]),
            linear_to_srgb(disp[2]),
        ];
        let y_s = luma709(srgb).clamp(0.0, 1.0);
        luma_srgb.push(y_s);

        let z = ((y_s * 10.999).floor() as usize).min(10);
        zones[z] += 1;

        if srgb[0] <= 0.004 && srgb[1] <= 0.004 && srgb[2] <= 0.004 {
            shadow_all += 1;
        }
        if srgb[0].max(srgb[1]).max(srgb[2]) >= 0.992 {
            hi_any += 1;
        }
        if srgb[0].min(srgb[1]).min(srgb[2]) >= 0.992 {
            hi_all += 1;
        }
        for c in 0..3 {
            if srgb[c] <= 0.004 {
                ch_sh[c] += 1;
            }
            if srgb[c] >= 0.992 {
                ch_hi[c] += 1;
            }
        }

        let (hsl, lab) = rgb_to_hsl_and_lab(srgb);
        let chroma = lab[1].hypot(lab[2]);
        chromas.push(chroma);
        sats.push(hsl[1]);
        labs.push((lab, srgb, hsl));

        all_a += lab[1] as f64;
        all_b += lab[2] as f64;
        if chroma <= 22.0 && (15.0..=90.0).contains(&lab[0]) {
            neu_a += lab[1] as f64;
            neu_b += lab[2] as f64;
            neu_n += 1;
        }

        let bucket = if lab[0] < 30.0 {
            0
        } else if lab[0] <= 70.0 {
            1
        } else {
            2
        };
        tone_ab[bucket][0] += lab[1] as f64;
        tone_ab[bucket][1] += lab[2] as f64;
        tone_n[bucket] += 1;

        if (10.0..=50.0).contains(&hsl[0])
            && (30.0..=85.0).contains(&lab[0])
            && (12.0..=55.0).contains(&chroma)
            && lab[1] > 4.0
            && lab[2] > 6.0
        {
            skin_lab[0] += lab[0] as f64;
            skin_lab[1] += lab[1] as f64;
            skin_lab[2] += lab[2] as f64;
            skin_n += 1;
        }
    }

    luma_srgb.sort_by(f32::total_cmp);
    luma_ev.sort_by(f32::total_cmp);
    chromas.sort_by(f32::total_cmp);
    sats.sort_by(f32::total_cmp);

    let p01_ev = percentile_sorted(&luma_ev, 0.01);
    let p99_ev = percentile_sorted(&luma_ev, 0.99);
    let p50_s = percentile_sorted(&luma_srgb, 0.50);
    let key = if p50_s < 0.30 {
        "low_key"
    } else if p50_s > 0.65 {
        "high_key"
    } else {
        "normal"
    }
    .to_owned();

    let mut zones_pct = [0.0_f32; 11];
    for i in 0..11 {
        zones_pct[i] = round2(zones[i] as f32 * 100.0 / nf);
    }

    let (cast_a, cast_b) = if neu_n >= 16 {
        ((neu_a / neu_n as f64) as f32, (neu_b / neu_n as f64) as f32)
    } else {
        ((all_a / nf as f64) as f32, (all_b / nf as f64) as f32)
    };
    let cast_note = describe_cast(cast_a, cast_b);

    let avg_tone = |i: usize| -> [f32; 2] {
        if tone_n[i] == 0 {
            [0.0, 0.0]
        } else {
            [
                round2((tone_ab[i][0] / tone_n[i] as f64) as f32),
                round2((tone_ab[i][1] / tone_n[i] as f64) as f32),
            ]
        }
    };

    let skin_metrics = if skin_n > 0 {
        let ml = (skin_lab[0] / skin_n as f64) as f32;
        let ma = (skin_lab[1] / skin_n as f64) as f32;
        let mb = (skin_lab[2] / skin_n as f64) as f32;
        let hue_deg = mb.atan2(ma).to_degrees();
        let note = if hue_deg < 45.0 {
            "magenta_cast"
        } else if hue_deg > 68.0 {
            "yellow_cast"
        } else {
            "natural_warm"
        }
        .to_owned();
        SkinToneMetrics {
            coverage_pct: round2(skin_n as f32 * 100.0 / nf),
            mean_lab: [round2(ml), round2(ma), round2(mb)],
            hue_deg: round2(hue_deg),
            note,
        }
    } else {
        SkinToneMetrics {
            coverage_pct: 0.0,
            mean_lab: [0.0, 0.0, 0.0],
            hue_deg: 0.0,
            note: "none_detected".to_owned(),
        }
    };

    let palette = extract_palette_kmeans(&labs, 5);
    let chroma_mean = chromas.iter().copied().sum::<f32>() / nf;
    let sat_mean = sats.iter().copied().sum::<f32>() / nf;

    let (auto_temp, auto_tint) = auto_white_balance(raw_proxy);
    let (auto_exp, auto_con, auto_hi, auto_sh) = auto_exposure_and_tone(raw_proxy);
    let auto_lvl = auto_levels(raw_proxy);

    let focus = detail_luma.map(|(luma, w, h)| compute_focus_metrics(luma, w, h));

    ImageAnalysis {
        tone: ToneAnalysis {
            luma_srgb_percentiles: LumaPercentiles {
                p01: round3(percentile_sorted(&luma_srgb, 0.01)),
                p05: round3(percentile_sorted(&luma_srgb, 0.05)),
                p25: round3(percentile_sorted(&luma_srgb, 0.25)),
                p50: round3(p50_s),
                p75: round3(percentile_sorted(&luma_srgb, 0.75)),
                p95: round3(percentile_sorted(&luma_srgb, 0.95)),
                p99: round3(percentile_sorted(&luma_srgb, 0.99)),
            },
            luma_ev_rel_18pct: EvPercentiles {
                p05: round2(percentile_sorted(&luma_ev, 0.05)),
                p50: round2(percentile_sorted(&luma_ev, 0.50)),
                p95: round2(percentile_sorted(&luma_ev, 0.95)),
                p99: round2(p99_ev),
            },
            dynamic_range_stops_p01_p99: round2((p99_ev - p01_ev).max(0.0)),
            key,
            hdr_headroom_pct: round2(hdr_headroom as f32 * 100.0 / nf),
            clip_pct: ClipMetrics {
                shadow_all: round2(shadow_all as f32 * 100.0 / nf),
                highlight_any: round2(hi_any as f32 * 100.0 / nf),
                highlight_all: round2(hi_all as f32 * 100.0 / nf),
                per_channel_shadow: [
                    round2(ch_sh[0] as f32 * 100.0 / nf),
                    round2(ch_sh[1] as f32 * 100.0 / nf),
                    round2(ch_sh[2] as f32 * 100.0 / nf),
                ],
                per_channel_highlight: [
                    round2(ch_hi[0] as f32 * 100.0 / nf),
                    round2(ch_hi[1] as f32 * 100.0 / nf),
                    round2(ch_hi[2] as f32 * 100.0 / nf),
                ],
            },
            zones_0_to_10_pct: zones_pct,
        },
        color: ColorAnalysis {
            cast_lab_neutral: NeutralCast {
                a_star: round2(cast_a),
                b_star: round2(cast_b),
                neutral_sample_pct: round2(neu_n as f32 * 100.0 / nf),
                note: cast_note,
            },
            cast_by_tone_ab: TonalCast {
                shadows: avg_tone(0),
                midtones: avg_tone(1),
                highlights: avg_tone(2),
            },
            chroma_mean: round2(chroma_mean),
            chroma_p95: round2(percentile_sorted(&chromas, 0.95)),
            saturation_mean_pct: round2(sat_mean),
            saturation_p95_pct: round2(percentile_sorted(&sats, 0.95)),
            skin_tones: skin_metrics,
            palette,
        },
        focus,
        suggested_auto: AutoSuggestions {
            exposure: round3(auto_exp),
            contrast: round3(auto_con),
            highlights: round3(auto_hi),
            shadows: round3(auto_sh),
            temperature: round3(auto_temp),
            tint: round3(auto_tint),
            levels_in_black: round3(auto_lvl.in_black),
            levels_in_mid: round3(auto_lvl.in_mid),
            levels_in_white: round3(auto_lvl.in_white),
        },
    }
}

/// Computes perceptual CIEDE2000, tonal, clipping, and recipe differences between two recipes.
pub fn diff_recipes(
    raw_proxy: &[[f32; 3]],
    recipe_a: &DevelopRecipe,
    recipe_b: &DevelopRecipe,
) -> RecipeDiff {
    let comp_a = recipe_a.compile();
    let comp_b = recipe_b.compile();
    let n = raw_proxy.len().max(1);
    let nf = n as f32;

    let mut des = Vec::with_capacity(n);
    let mut sh_de = (0.0_f64, 0usize);
    let mut mid_de = (0.0_f64, 0usize);
    let mut hi_de = (0.0_f64, 0usize);
    let mut skin_de = (0.0_f64, 0usize);

    let mut delta_ev_sum = 0.0_f64;
    let mut lstar_a = Vec::with_capacity(n);
    let mut lstar_b = Vec::with_capacity(n);

    let mut sh_a = 0usize;
    let mut sh_b = 0usize;
    let mut hi_a = 0usize;
    let mut hi_b = 0usize;
    let mut recovered_hi = 0usize;
    let mut newly_hi = 0usize;

    let mut neu_da = 0.0_f64;
    let mut neu_db = 0.0_f64;
    let mut neu_n = 0usize;
    let mut chroma_a_sum = 0.0_f64;
    let mut chroma_b_sum = 0.0_f64;

    for &px in raw_proxy {
        let sa = comp_a.evaluate_pixel_scene_linear(px);
        let sb = comp_b.evaluate_pixel_scene_linear(px);
        let ev_a = (luma709(sa).max(1e-5) / 0.18).log2();
        let ev_b = (luma709(sb).max(1e-5) / 0.18).log2();
        delta_ev_sum += (ev_b - ev_a) as f64;

        let da = filmic_tonemap_with_curve(sa, comp_a.film_curve);
        let db = filmic_tonemap_with_curve(sb, comp_b.film_curve);
        let srgb_a = [
            linear_to_srgb(da[0]),
            linear_to_srgb(da[1]),
            linear_to_srgb(da[2]),
        ];
        let srgb_b = [
            linear_to_srgb(db[0]),
            linear_to_srgb(db[1]),
            linear_to_srgb(db[2]),
        ];

        let is_sh_a = srgb_a[0] <= 0.004 && srgb_a[1] <= 0.004 && srgb_a[2] <= 0.004;
        let is_sh_b = srgb_b[0] <= 0.004 && srgb_b[1] <= 0.004 && srgb_b[2] <= 0.004;
        let is_hi_a = srgb_a[0].max(srgb_a[1]).max(srgb_a[2]) >= 0.992;
        let is_hi_b = srgb_b[0].max(srgb_b[1]).max(srgb_b[2]) >= 0.992;
        if is_sh_a {
            sh_a += 1;
        }
        if is_sh_b {
            sh_b += 1;
        }
        if is_hi_a {
            hi_a += 1;
        }
        if is_hi_b {
            hi_b += 1;
        }
        if is_hi_a && !is_hi_b {
            recovered_hi += 1;
        }
        if !is_hi_a && is_hi_b {
            newly_hi += 1;
        }

        let (hsl_a, lab_a) = rgb_to_hsl_and_lab(srgb_a);
        let (_, lab_b) = rgb_to_hsl_and_lab(srgb_b);
        lstar_a.push(lab_a[0]);
        lstar_b.push(lab_b[0]);

        let de = ciede2000(lab_a, lab_b);
        des.push(de);

        if lab_a[0] < 30.0 {
            sh_de.0 += de as f64;
            sh_de.1 += 1;
        } else if lab_a[0] <= 70.0 {
            mid_de.0 += de as f64;
            mid_de.1 += 1;
        } else {
            hi_de.0 += de as f64;
            hi_de.1 += 1;
        }

        let ca = lab_a[1].hypot(lab_a[2]);
        let cb = lab_b[1].hypot(lab_b[2]);
        chroma_a_sum += ca as f64;
        chroma_b_sum += cb as f64;

        if ca <= 22.0 && (15.0..=90.0).contains(&lab_a[0]) {
            neu_da += (lab_b[1] - lab_a[1]) as f64;
            neu_db += (lab_b[2] - lab_a[2]) as f64;
            neu_n += 1;
        }

        if (10.0..=50.0).contains(&hsl_a[0])
            && (30.0..=85.0).contains(&lab_a[0])
            && (12.0..=55.0).contains(&ca)
            && lab_a[1] > 4.0
            && lab_a[2] > 6.0
        {
            skin_de.0 += de as f64;
            skin_de.1 += 1;
        }
    }

    des.sort_by(f32::total_cmp);
    lstar_a.sort_by(f32::total_cmp);
    lstar_b.sort_by(f32::total_cmp);

    let mean_de = des.iter().copied().sum::<f32>() / nf;
    let avg = |acc: (f64, usize)| -> f32 {
        if acc.1 == 0 {
            0.0
        } else {
            round2((acc.0 / acc.1 as f64) as f32)
        }
    };

    RecipeDiff {
        delta_e2000: DeltaEStats {
            mean: round2(mean_de),
            p95: round2(percentile_sorted(&des, 0.95)),
            max: round2(des.last().copied().unwrap_or(0.0)),
            shadows: avg(sh_de),
            midtones: avg(mid_de),
            highlights: avg(hi_de),
            skin_tones: avg(skin_de),
        },
        luma_shift: LumaShift {
            mean_delta_ev: round3((delta_ev_sum / nf as f64) as f32),
            p50_delta_lstar: round2(
                percentile_sorted(&lstar_b, 0.50) - percentile_sorted(&lstar_a, 0.50),
            ),
        },
        clip_delta_pct: ClipDelta {
            shadow_all: round2((sh_b as f32 - sh_a as f32) * 100.0 / nf),
            highlight_any: round2((hi_b as f32 - hi_a as f32) * 100.0 / nf),
            recovered_highlights: round2(recovered_hi as f32 * 100.0 / nf),
            newly_clipped_highlights: round2(newly_hi as f32 * 100.0 / nf),
        },
        color_shift: ColorShift {
            delta_a_star_neutral: if neu_n > 0 {
                round2((neu_da / neu_n as f64) as f32)
            } else {
                0.0
            },
            delta_b_star_neutral: if neu_n > 0 {
                round2((neu_db / neu_n as f64) as f32)
            } else {
                0.0
            },
            chroma_ratio: if chroma_a_sum > 1e-4 {
                round3((chroma_b_sum / chroma_a_sum) as f32)
            } else {
                1.0
            },
        },
        recipe_changes: recipe_field_diffs(recipe_a, recipe_b),
    }
}

/// Exact Sharma, Wu & Dalal (2005) CIEDE2000 perceptual color difference (`kL=kC=kH=1`).
pub fn ciede2000(lab1: [f32; 3], lab2: [f32; 3]) -> f32 {
    let (l1, a1, b1) = (lab1[0] as f64, lab1[1] as f64, lab1[2] as f64);
    let (l2, a2, b2) = (lab2[0] as f64, lab2[1] as f64, lab2[2] as f64);

    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let c_bar = 0.5 * (c1 + c2);
    let c_bar7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c_bar7 / (c_bar7 + 25.0_f64.powi(7))).sqrt());

    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = a1p.hypot(b1);
    let c2p = a2p.hypot(b2);

    let hp = |b: f64, ap: f64| -> f64 {
        if b == 0.0 && ap == 0.0 {
            0.0
        } else {
            b.atan2(ap).to_degrees().rem_euclid(360.0)
        }
    };
    let h1p = hp(b1, a1p);
    let h2p = hp(b2, a2p);

    let d_lp = l2 - l1;
    let d_cp = c2p - c1p;
    let d_hp = if c1p * c2p == 0.0 {
        0.0
    } else {
        let diff = h2p - h1p;
        if diff.abs() <= 180.0 {
            diff
        } else if diff > 180.0 {
            diff - 360.0
        } else {
            diff + 360.0
        }
    };
    let d_big_hp = 2.0 * (c1p * c2p).sqrt() * (d_hp * 0.5).to_radians().sin();

    let l_bar_p = 0.5 * (l1 + l2);
    let c_bar_p = 0.5 * (c1p + c2p);
    let h_bar_p = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        0.5 * (h1p + h2p)
    } else if h1p + h2p < 360.0 {
        0.5 * (h1p + h2p + 360.0)
    } else {
        0.5 * (h1p + h2p - 360.0)
    };

    let t = 1.0 - 0.17 * (h_bar_p - 30.0).to_radians().cos()
        + 0.24 * (2.0 * h_bar_p).to_radians().cos()
        + 0.32 * (3.0 * h_bar_p + 6.0).to_radians().cos()
        - 0.20 * (4.0 * h_bar_p - 63.0).to_radians().cos();

    let d_theta = 30.0 * (-((h_bar_p - 275.0) / 25.0).powi(2)).exp();
    let c_bar_p7 = c_bar_p.powi(7);
    let r_c = 2.0 * (c_bar_p7 / (c_bar_p7 + 25.0_f64.powi(7))).sqrt();
    let l50_sq = (l_bar_p - 50.0).powi(2);
    let s_l = 1.0 + (0.015 * l50_sq) / (20.0 + l50_sq).sqrt();
    let s_c = 1.0 + 0.045 * c_bar_p;
    let s_h = 1.0 + 0.015 * c_bar_p * t;
    let r_t = -(2.0 * d_theta).to_radians().sin() * r_c;

    let dl = d_lp / s_l;
    let dc = d_cp / s_c;
    let dh = d_big_hp / s_h;
    (dl * dl + dc * dc + dh * dh + r_t * dc * dh)
        .max(0.0)
        .sqrt() as f32
}

/// Computes sharpness, acutance focus mask %, flat-area noise, and 3x3 spatial grids
/// from a perceptual luma (`sqrt(luma709(linear))`) buffer.
pub fn compute_focus_metrics(luma: &[f32], width: u32, height: u32) -> FocusAnalysis {
    let w = width as usize;
    let h = height as usize;
    if w < 3 || h < 3 || luma.len() < w * h {
        return FocusAnalysis {
            sharpness_score: 0.0,
            laplacian_var: 0.0,
            tenengrad: 0.0,
            focus_mask_pct: 0.0,
            flat_noise_est: 0.0,
            sharpness_zones_3x3: [[0.0; 3]; 3],
            luma_zones_3x3: [[0.0; 3]; 3],
        };
    }

    let mut lap_sum = 0.0_f64;
    let mut lap_sq_sum = 0.0_f64;
    let mut tenengrad_sum = 0.0_f64;
    let mut focus_hits = 0usize;
    let mut count = 0usize;
    let mut flat_laps = Vec::new();

    let mut zone_ten = [[0.0_f64; 3]; 3];
    let mut zone_luma = [[0.0_f64; 3]; 3];
    let mut zone_n = [[0usize; 3]; 3];

    for y in 1..(h - 1) {
        let zy = ((y * 3) / h).min(2);
        for x in 1..(w - 1) {
            let zx = ((x * 3) / w).min(2);
            let idx = y * w + x;
            let c = luma[idx];
            let l = luma[idx - 1];
            let r = luma[idx + 1];
            let u = luma[idx - w];
            let d = luma[idx + w];

            let gx = r - l;
            let gy = d - u;
            let grad_sq = (gx * gx + gy * gy) as f64;
            let grad = (gx * gx + gy * gy).sqrt();
            let lap = 4.0 * c - (l + r + u + d);
            let lap_abs = lap.abs();

            lap_sum += lap as f64;
            lap_sq_sum += (lap * lap) as f64;
            tenengrad_sum += grad_sq;
            count += 1;

            let acutance = grad + lap_abs * 0.5;
            let t = ((acutance - 0.065) / (0.16 - 0.065)).clamp(0.0, 1.0);
            let focus_alpha = t * t * (3.0 - 2.0 * t);
            if focus_alpha > 0.25 {
                focus_hits += 1;
            }
            if grad < 0.02 {
                flat_laps.push(lap_abs);
            }

            zone_ten[zy][zx] += grad_sq;
            zone_luma[zy][zx] += (c * c * 100.0) as f64;
            zone_n[zy][zx] += 1;
        }
    }

    let nf = count.max(1) as f64;
    let lap_mean = lap_sum / nf;
    let lap_var = ((lap_sq_sum / nf) - lap_mean * lap_mean).max(0.0) as f32;
    let tenengrad = (tenengrad_sum / nf) as f32;
    let sharpness_score = round2((tenengrad.sqrt() * 500.0).clamp(0.0, 100.0));

    flat_laps.sort_by(f32::total_cmp);
    let flat_noise_est = if flat_laps.is_empty() {
        0.0
    } else {
        (percentile_sorted(&flat_laps, 0.50) * 10_000.0).round() / 10_000.0
    };

    let mut sharp_grid = [[0.0_f32; 3]; 3];
    let mut luma_grid = [[0.0_f32; 3]; 3];
    for zy in 0..3 {
        for zx in 0..3 {
            let zn = zone_n[zy][zx].max(1) as f64;
            sharp_grid[zy][zx] =
                round2((((zone_ten[zy][zx] / zn).sqrt() * 500.0) as f32).clamp(0.0, 100.0));
            luma_grid[zy][zx] = round2((zone_luma[zy][zx] / zn) as f32);
        }
    }

    FocusAnalysis {
        sharpness_score,
        laplacian_var: (lap_var * 100_000.0).round() / 100_000.0,
        tenengrad: (tenengrad * 100_000.0).round() / 100_000.0,
        focus_mask_pct: round2(focus_hits as f32 * 100.0 / count.max(1) as f32),
        flat_noise_est,
        sharpness_zones_3x3: sharp_grid,
        luma_zones_3x3: luma_grid,
    }
}

fn describe_cast(a: f32, b: f32) -> String {
    if a.abs() < 2.0 && b.abs() < 2.5 {
        return "neutral".to_owned();
    }
    let mut parts = Vec::new();
    if b >= 2.5 {
        parts.push("warm_yellow");
    } else if b <= -2.5 {
        parts.push("cool_blue");
    }
    if a >= 2.0 {
        parts.push("magenta");
    } else if a <= -2.0 {
        parts.push("green");
    }
    parts.join("_")
}

fn extract_palette_kmeans(
    samples: &[([f32; 3], [f32; 3], [f32; 3])],
    k: usize,
) -> Vec<PaletteSwatch> {
    if samples.is_empty() || k == 0 {
        return Vec::new();
    }
    let stride = (samples.len() / 2048).max(1);
    let sub: Vec<_> = samples.iter().step_by(stride).collect();
    let k = k.min(sub.len());

    // Deterministic furthest-point seeding starting from pixel closest to L*=50.
    let mut seeds = Vec::with_capacity(k);
    let first = sub
        .iter()
        .min_by(|a, b| (a.0[0] - 50.0).abs().total_cmp(&(b.0[0] - 50.0).abs()))
        .unwrap();
    seeds.push(first.0);

    while seeds.len() < k {
        let mut best_lab = sub[0].0;
        let mut best_dist = -1.0_f32;
        for s in &sub {
            let mut min_d = f32::MAX;
            for c in &seeds {
                let d = sq_dist_lab(s.0, *c);
                if d < min_d {
                    min_d = d;
                }
            }
            if min_d > best_dist {
                best_dist = min_d;
                best_lab = s.0;
            }
        }
        seeds.push(best_lab);
    }

    let mut centroids = seeds;
    let mut assigns = vec![0usize; sub.len()];
    for _ in 0..8 {
        let mut sums = vec![[0.0_f64; 3]; k];
        let mut counts = vec![0usize; k];
        for (i, s) in sub.iter().enumerate() {
            let mut best = 0;
            let mut best_d = f32::MAX;
            for (ci, c) in centroids.iter().enumerate() {
                let d = sq_dist_lab(s.0, *c);
                if d < best_d {
                    best_d = d;
                    best = ci;
                }
            }
            assigns[i] = best;
            sums[best][0] += s.0[0] as f64;
            sums[best][1] += s.0[1] as f64;
            sums[best][2] += s.0[2] as f64;
            counts[best] += 1;
        }
        for ci in 0..k {
            if counts[ci] > 0 {
                centroids[ci] = [
                    (sums[ci][0] / counts[ci] as f64) as f32,
                    (sums[ci][1] / counts[ci] as f64) as f32,
                    (sums[ci][2] / counts[ci] as f64) as f32,
                ];
            }
        }
    }

    let mut rgb_sums = vec![[0.0_f64; 3]; k];
    let mut counts = vec![0usize; k];
    for (i, s) in sub.iter().enumerate() {
        let ci = assigns[i];
        rgb_sums[ci][0] += s.1[0] as f64;
        rgb_sums[ci][1] += s.1[1] as f64;
        rgb_sums[ci][2] += s.1[2] as f64;
        counts[ci] += 1;
    }

    let total = sub.len().max(1) as f32;
    let mut out: Vec<PaletteSwatch> = (0..k)
        .filter(|&ci| counts[ci] > 0)
        .map(|ci| {
            let srgb = [
                (rgb_sums[ci][0] / counts[ci] as f64) as f32,
                (rgb_sums[ci][1] / counts[ci] as f64) as f32,
                (rgb_sums[ci][2] / counts[ci] as f64) as f32,
            ];
            let (hsl, lab) = rgb_to_hsl_and_lab(srgb);
            let r8 = (srgb[0].clamp(0.0, 1.0) * 255.0).round() as u8;
            let g8 = (srgb[1].clamp(0.0, 1.0) * 255.0).round() as u8;
            let b8 = (srgb[2].clamp(0.0, 1.0) * 255.0).round() as u8;
            let band = if hsl[1] < 10.0 {
                "Neutral".to_owned()
            } else {
                nearest_hsl_band(hsl[0]).to_owned()
            };
            PaletteSwatch {
                hex: format!("#{r8:02X}{g8:02X}{b8:02X}"),
                pct: round2(counts[ci] as f32 * 100.0 / total),
                hsl: [round2(hsl[0]), round2(hsl[1]), round2(hsl[2])],
                lab: [round2(lab[0]), round2(lab[1]), round2(lab[2])],
                band,
            }
        })
        .collect();
    out.sort_by(|a, b| b.pct.total_cmp(&a.pct));
    out
}

fn sq_dist_lab(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dl = a[0] - b[0];
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    dl * dl + da * da + db * db
}

fn nearest_hsl_band(hue_deg: f32) -> &'static str {
    let h = hue_deg.rem_euclid(360.0);
    let mut best_idx = 0;
    let mut best_dist = f32::MAX;
    for (i, &center) in HSL_BAND_HUES.iter().enumerate() {
        let diff = (h - center).abs();
        let dist = diff.min(360.0 - diff);
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }
    HSL_BAND_NAMES[best_idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciede2000_matches_sharma_reference_pair() {
        // Sharma et al. (2005) Table 1 Pair #1: (50, 2.6772, -79.7751) vs (50, 0, -82.7485) -> 2.0425
        let de = ciede2000([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485]);
        assert!((de - 2.0425).abs() < 0.01, "expected ~2.0425, got {de}");
        assert!(ciede2000([50.0, 10.0, 20.0], [50.0, 10.0, 20.0]) < 1e-5);
    }

    #[test]
    fn analyze_and_diff_produce_valid_metrics() {
        let proxy: Vec<[f32; 3]> = (0..256)
            .map(|i| {
                let v = (i as f32 + 1.0) / 256.0;
                [v * 0.45, v * 0.35, v * 0.25]
            })
            .collect();
        let neutral = DevelopRecipe::default();
        let mut warm = neutral.clone();
        warm.exposure = 0.5;
        warm.temperature = 0.2;

        let analysis = analyze_linear_proxy(&proxy, &warm, None);
        assert!(analysis.tone.dynamic_range_stops_p01_p99 > 1.0);
        assert!(!analysis.color.palette.is_empty());

        let diff = diff_recipes(&proxy, &neutral, &warm);
        assert!(diff.delta_e2000.mean > 2.0);
        assert!(diff.luma_shift.mean_delta_ev > 0.3);
        assert_eq!(diff.recipe_changes.len(), 2);
    }
}
