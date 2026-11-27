//! Portable `wgpu` linear GPU photo develop engine.
//!
//! Designed to execute inside Rune's `Compositor<S>` (`EffectSite` slot) directly
//! on macOS `objc2-metal` (via Rune's custom Metal backend) as well as DX12/Vulkan.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use rune_editor_core::{
    ADVANCED_COLOR_SLICE_COUNT, CURVE_LUT_SIZE, DevelopRecipe, HSL_BAND_COUNT,
    LAYER_MASK_ATLAS_HEIGHT, LAYER_MASK_ATLAS_WIDTH, LayerMaskAtlas, LayerMaskItem,
    MAX_ADJUSTMENT_LAYERS, MAX_HEAL_SPOTS, PROFILE_LUT_SIZE, ProfileLut3D, ToneCurves,
    ViewportCamera, bake_layer_mask_atlas, canonical_profile_id, resolve_profile_lut,
};
use rune_editor_decode::LinearImage;

const DEVELOP_WGSL: &str = include_str!("../../../shaders/develop.wgsl");

/// Strictly 16-byte aligned uniform block matching `DevelopUniforms` in `shaders/develop.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DevelopUniforms {
    /// `[slot_x, slot_y, slot_w, slot_h]` in pass target device pixels.
    pub slot_rect: [f32; 4],
    /// `[target_w, target_h, image_w, image_h]`
    pub dims: [f32; 4],
    /// `[zoom, pan_x, pan_y, rotation_rad]`
    pub camera: [f32; 4],
    /// `[exposure, contrast, highlights, shadows]`
    pub tone_a: [f32; 4],
    /// `[whites, blacks, temperature, tint]`
    pub tone_b: [f32; 4],
    /// `[vibrance, saturation, vignette, grain]`
    pub color_fx: [f32; 4],
    /// `[split_enabled, split_pos, show_original, srgb_encode]`
    pub flags: [f32; 4],
    /// Capture One 3-Way Shadows wheel `[hue_x, hue_y, luma, 0.0]`.
    pub shadows_wheel: [f32; 4],
    /// Capture One 3-Way Midtones wheel `[hue_x, hue_y, luma, 0.0]`.
    pub midtones_wheel: [f32; 4],
    /// Capture One 3-Way Highlights wheel `[hue_x, hue_y, luma, 0.0]`.
    pub highlights_wheel: [f32; 4],
    /// 8-Band HSL Color Editor packed as `[hue, saturation, luminance, 0.0]`.
    pub hsl: [[f32; 4]; HSL_BAND_COUNT],
    /// Spatial detail controls `[clarity, sharpen, noise_reduction, structure]`.
    pub detail: [f32; 4],
    /// Tone & atmosphere `[brightness, dehaze, levels.out_black, levels.out_white]`.
    pub atmosphere: [f32; 4],
    /// Levels input controls `[in_black, in_mid, in_white, levels_active (0/1)]`.
    pub levels: [f32; 4],
    /// 6-Channel Black & White mixer `[[enabled, red, yellow, green], [cyan, blue, magenta, 0.0]]`.
    pub bw_channels: [[f32; 4]; 2],
    /// Normalized crop rectangle `[x, y, w, h]` in oriented-frame coordinates.
    pub geo_crop: [f32; 4],
    /// Persistent geometry transform `[straighten_rad, rotate_90 (0..3), flip_h, flip_v]`.
    pub geo_xform: [f32; 4],
    /// Viewer overlays `[exposure_warning, focus_mask, crop_mode, grid_mode]`.
    pub overlay_flags: [f32; 4],
    /// Capture One Color Balance Master wheel `[hue_x, hue_y, luma, 0.0]`.
    pub master_wheel: [f32; 4],
    /// B&W Split Toning `[split_shadow_hue, split_shadow_sat, split_highlight_hue, split_highlight_sat]`.
    pub bw_split: [f32; 4],
    /// Per-channel Red Levels input `[in_black, in_mid, in_white, r_active (0/1)]`.
    pub levels_r_in: [f32; 4],
    /// Per-channel Green Levels input `[in_black, in_mid, in_white, g_active (0/1)]`.
    pub levels_g_in: [f32; 4],
    /// Per-channel Blue Levels input `[in_black, in_mid, in_white, b_active (0/1)]`.
    pub levels_b_in: [f32; 4],
    /// Per-channel R/G Levels output `[r.out_black, r.out_white, g.out_black, g.out_white]`.
    pub levels_rgb_out: [f32; 4],
    /// `[b.out_black, b.out_white, film_curve (0..3), master_levels_active (0/1)]`.
    pub levels_b_out_curve: [f32; 4],
    /// Extended spatial detail `[sharpen_radius, sharpen_threshold, sharpen_halo, nr_color]`.
    pub detail_ext: [f32; 4],
    /// Mode selectors & grain size `[clarity_method (0..3), grain_type (0..5), grain_size (0..1), vignette_method (0..2)]`.
    pub style_modes: [f32; 4],
    /// Advanced Color Editor (4 slices × 3 `vec4<f32>`).
    pub advanced_color: [[f32; 4]; ADVANCED_COLOR_SLICE_COUNT * 3],
    /// Skin Tone Uniformity (3 `vec4<f32>`).
    pub skin_tone: [[f32; 4]; 3],
    /// Optical Lens Correction `[distortion, chromatic_aberration, purple_fringing, light_falloff]`.
    pub lens: [f32; 4],
    /// Perspective Keystone `[keystone_vertical, keystone_horizontal, keystone_aspect, keystone_amount]`.
    pub geo_keystone: [f32; 4],
    /// Transient viewport flags `[view_color_range (-1..4), loupe_enabled (0/1), 0.0, 0.0]`.
    pub viewport_extra: [f32; 4],
    /// 120Hz GPU Loupe overlay `[loupe_u (0..1), loupe_v (0..1), loupe_zoom (1.25..16), loupe_radius_px]`.
    pub loupe_params: [f32; 4],
    /// Mask overlay & counts `[mask_overlay_mode (0/1/2), active_layer_index (-1..7), layer_count (0..8), heal_count (0..16)]`.
    pub mask_overlay: [f32; 4],
    /// Per-layer metadata (8 layers): `[enabled (0/1), opacity (0..1), inverted (0/1), has_spatial (0/1)]`.
    pub layer_meta: [[f32; 4]; MAX_ADJUSTMENT_LAYERS],
    /// Per-layer local adjustments (8 layers × 3 `vec4<f32>`).
    pub layer_adj: [[f32; 4]; MAX_ADJUSTMENT_LAYERS * 3],
    /// Per-layer live Luma Range params `[low, high, low_falloff, high_falloff]`.
    pub layer_luma_a: [[f32; 4]; MAX_ADJUSTMENT_LAYERS],
    /// Per-layer live Luma Range flags `[enabled (0/1), op (0/1/2), inverted (0/1), 0.0]`.
    pub layer_luma_b: [[f32; 4]; MAX_ADJUSTMENT_LAYERS],
    /// Spot Heal & Clone stamps (16 spots × 2 `vec4<f32>`).
    pub heal_spots: [[f32; 4]; MAX_HEAL_SPOTS * 2],
    /// Film Negative Conversion `[base_r, base_g, base_b, processing_mode (0/1)]`.
    pub film_neg_a: [f32; 4],
    /// Film Negative Conversion `[density, contrast, black_clip, white_clip]`.
    pub film_neg_b: [f32; 4],
    /// Camera Profile / Film Simulation 3D LUT `[profile_active (0/1), profile_strength (0..1), 0.0, 0.0]`.
    pub profile_params: [f32; 4],
}

impl DevelopUniforms {
    pub fn build(
        slot_rect: [f32; 4],
        target_size: [u32; 2],
        image_size: [u32; 2],
        camera: &ViewportCamera,
        recipe: &DevelopRecipe,
        srgb_blend: bool,
    ) -> Self {
        Self::build_with_profiles_dir(
            slot_rect,
            target_size,
            image_size,
            camera,
            recipe,
            srgb_blend,
            None,
        )
    }

    pub fn build_with_profiles_dir(
        slot_rect: [f32; 4],
        target_size: [u32; 2],
        image_size: [u32; 2],
        camera: &ViewportCamera,
        recipe: &DevelopRecipe,
        srgb_blend: bool,
        profiles_dir: Option<&Path>,
    ) -> Self {
        let mut hsl = [[0.0_f32; 4]; HSL_BAND_COUNT];
        for (dst, band) in hsl.iter_mut().zip(recipe.hsl.iter()) {
            *dst = band.to_uniform();
        }
        let clamp_unit = |v: f32| {
            if v.is_finite() {
                v.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        };
        let clamp_pos = |v: f32, default_v: f32, min_v: f32, max_v: f32| {
            if v.is_finite() {
                v.clamp(min_v, max_v)
            } else {
                default_v
            }
        };
        let levels = recipe.levels.sanitized();
        let levels_active = if levels.is_identity() { 0.0 } else { 1.0 };
        let master_levels_active = if levels.is_master_identity() {
            0.0
        } else {
            1.0
        };
        let r_active = if levels.red.is_identity() { 0.0 } else { 1.0 };
        let g_active = if levels.green.is_identity() { 0.0 } else { 1.0 };
        let b_active = if levels.blue.is_identity() { 0.0 } else { 1.0 };

        let mut advanced_color = [[0.0_f32; 4]; ADVANCED_COLOR_SLICE_COUNT * 3];
        for (i, slice) in recipe.advanced_color.iter().enumerate() {
            let packed = slice.to_uniforms();
            advanced_color[i * 3] = packed[0];
            advanced_color[i * 3 + 1] = packed[1];
            advanced_color[i * 3 + 2] = packed[2];
        }

        let layer_count = recipe.layers.len().min(MAX_ADJUSTMENT_LAYERS);
        let heal_count = recipe.heal_spots.len().min(MAX_HEAL_SPOTS);
        let mut layer_meta = [[0.0_f32; 4]; MAX_ADJUSTMENT_LAYERS];
        let mut layer_adj = [[0.0_f32; 4]; MAX_ADJUSTMENT_LAYERS * 3];
        let mut layer_luma_a = [[0.0_f32; 4]; MAX_ADJUSTMENT_LAYERS];
        let mut layer_luma_b = [[0.0_f32; 4]; MAX_ADJUSTMENT_LAYERS];

        for (i, layer) in recipe.layers.iter().take(MAX_ADJUSTMENT_LAYERS).enumerate() {
            let clean = layer.sanitized();
            let mut has_spatial = false;
            let mut luma_spec = rune_editor_core::LayerLumaRangeSpec::default();
            for item in &clean.masks {
                if !item.enabled {
                    continue;
                }
                if let rune_editor_core::MaskShape::LumaRange {
                    low,
                    high,
                    low_falloff,
                    high_falloff,
                } = item.shape
                {
                    if !luma_spec.enabled {
                        luma_spec = rune_editor_core::LayerLumaRangeSpec {
                            enabled: true,
                            low,
                            high,
                            low_falloff,
                            high_falloff,
                            op: item.op,
                            inverted: item.inverted,
                        };
                    }
                } else {
                    has_spatial = true;
                }
            }
            layer_meta[i] = [
                if clean.enabled { 1.0 } else { 0.0 },
                clean.opacity,
                if clean.inverted { 1.0 } else { 0.0 },
                if has_spatial { 1.0 } else { 0.0 },
            ];
            let adj_u = clean.adjustments.to_uniforms();
            layer_adj[i * 3] = adj_u[0];
            layer_adj[i * 3 + 1] = adj_u[1];
            layer_adj[i * 3 + 2] = adj_u[2];
            let (la, lb) = luma_spec.to_uniforms();
            layer_luma_a[i] = la;
            layer_luma_b[i] = lb;
        }

        let mut heal_spots = [[0.0_f32; 4]; MAX_HEAL_SPOTS * 2];
        for (i, spot) in recipe.heal_spots.iter().take(MAX_HEAL_SPOTS).enumerate() {
            let packed = spot.to_uniforms();
            heal_spots[i * 2] = packed[0];
            heal_spots[i * 2 + 1] = packed[1];
        }
        let (film_neg_a, film_neg_b) = recipe.film_negative.to_uniforms(recipe.processing_mode);
        let profile_strength = clamp_pos(recipe.profile_strength, 1.0, 0.0, 1.0);
        let profile_active = if profile_strength > 1e-5
            && resolve_profile_lut(&recipe.camera_profile, profiles_dir).is_some()
        {
            1.0
        } else {
            0.0
        };
        let profile_params = [profile_active, profile_strength, 0.0, 0.0];

        Self {
            slot_rect,
            dims: [
                target_size[0] as f32,
                target_size[1] as f32,
                image_size[0] as f32,
                image_size[1] as f32,
            ],
            camera: [
                camera.zoom,
                camera.pan[0],
                camera.pan[1],
                camera.rotation_deg.to_radians(),
            ],
            tone_a: [
                recipe.exposure,
                recipe.contrast,
                recipe.highlights,
                recipe.shadows,
            ],
            tone_b: [
                recipe.whites,
                recipe.blacks,
                recipe.temperature,
                recipe.tint,
            ],
            color_fx: [
                recipe.vibrance,
                recipe.saturation,
                recipe.vignette,
                recipe.grain,
            ],
            flags: [
                if camera.split_compare { 1.0 } else { 0.0 },
                camera.split_pos,
                if camera.show_original { 1.0 } else { 0.0 },
                if srgb_blend { 1.0 } else { 0.0 },
            ],
            shadows_wheel: recipe.shadows_wheel.clamped().to_uniform(),
            midtones_wheel: recipe.midtones_wheel.clamped().to_uniform(),
            highlights_wheel: recipe.highlights_wheel.clamped().to_uniform(),
            hsl,
            detail: [
                recipe.clarity,
                recipe.sharpen,
                recipe.noise_reduction,
                clamp_unit(recipe.structure),
            ],
            atmosphere: [
                clamp_unit(recipe.brightness),
                clamp_unit(recipe.dehaze),
                levels.out_black,
                levels.out_white,
            ],
            levels: [
                levels.in_black,
                levels.in_mid,
                levels.in_white,
                levels_active,
            ],
            bw_channels: recipe.bw.to_uniforms(),
            geo_crop: recipe.geometry.sanitized_crop(),
            geo_xform: recipe.geometry.to_xform_uniform(),
            overlay_flags: [
                if camera.exposure_warning { 1.0 } else { 0.0 },
                if camera.focus_mask { 1.0 } else { 0.0 },
                if camera.crop_mode { 1.0 } else { 0.0 },
                recipe.geometry.grid.to_uniform_f32(),
            ],
            master_wheel: recipe.master_wheel.clamped().to_uniform(),
            bw_split: recipe.bw.to_split_uniform(),
            levels_r_in: [
                levels.red.in_black,
                levels.red.in_mid,
                levels.red.in_white,
                r_active,
            ],
            levels_g_in: [
                levels.green.in_black,
                levels.green.in_mid,
                levels.green.in_white,
                g_active,
            ],
            levels_b_in: [
                levels.blue.in_black,
                levels.blue.in_mid,
                levels.blue.in_white,
                b_active,
            ],
            levels_rgb_out: [
                levels.red.out_black,
                levels.red.out_white,
                levels.green.out_black,
                levels.green.out_white,
            ],
            levels_b_out_curve: [
                levels.blue.out_black,
                levels.blue.out_white,
                recipe.film_curve.to_uniform_f32(),
                master_levels_active,
            ],
            detail_ext: [
                clamp_pos(recipe.sharpen_radius, 1.0, 0.5, 4.0),
                clamp_pos(recipe.sharpen_threshold, 0.0, 0.0, 1.0),
                clamp_pos(recipe.sharpen_halo, 0.0, 0.0, 1.0),
                clamp_pos(recipe.nr_color, 0.0, 0.0, 1.0),
            ],
            style_modes: [
                recipe.clarity_method.to_uniform_f32(),
                recipe.grain_type.to_uniform_f32(),
                clamp_pos(recipe.grain_size, 0.35, 0.0, 1.0),
                recipe.vignette_method.to_uniform_f32(),
            ],
            advanced_color,
            skin_tone: recipe.skin_tone.to_uniforms(),
            lens: recipe.lens.to_uniform(),
            geo_keystone: recipe.geometry.to_keystone_uniform(),
            viewport_extra: [
                camera.view_color_range.clamp(-1, 4) as f32,
                if camera.loupe_enabled { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
            loupe_params: [
                clamp_pos(camera.loupe_uv[0], 0.5, 0.0, 1.0),
                clamp_pos(camera.loupe_uv[1], 0.5, 0.0, 1.0),
                clamp_pos(camera.loupe_zoom, 2.5, 1.25, 16.0),
                clamp_pos(camera.loupe_radius_px, 96.0, 24.0, 320.0),
            ],
            mask_overlay: [
                camera.mask_overlay_mode.clamp(0, 2) as f32,
                camera
                    .active_layer_index
                    .clamp(-1, (MAX_ADJUSTMENT_LAYERS as i32) - 1) as f32,
                layer_count as f32,
                heal_count as f32,
            ],
            layer_meta,
            layer_adj,
            layer_luma_a,
            layer_luma_b,
            heal_spots,
            film_neg_a,
            film_neg_b,
            profile_params,
        }
    }
}

/// GPU resources owned by the develop viewport for a single `wgpu::Device`.
pub struct DevelopGpuResources {
    shader: wgpu::ShaderModule,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    uniform_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    curve_lut_texture: wgpu::Texture,
    curve_lut_view: wgpu::TextureView,
    uploaded_curves: Option<ToneCurves>,
    layer_mask_atlas_texture: wgpu::Texture,
    layer_mask_atlas_view: wgpu::TextureView,
    uploaded_layer_masks: Option<Vec<Vec<LayerMaskItem>>>,
    profile_lut_texture: wgpu::Texture,
    profile_lut_view: wgpu::TextureView,
    uploaded_profile_lut: Option<(String, Option<Arc<ProfileLut3D>>)>,
    photo_texture: Option<wgpu::Texture>,
    bind_group: Option<wgpu::BindGroup>,
    uploaded_generation: u64,
    uploaded_size: [u32; 2],
}

impl DevelopGpuResources {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rune-editor develop.wgsl"),
            source: wgpu::ShaderSource::Wgsl(DEVELOP_WGSL.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rune-editor develop bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rune-editor develop layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rune-editor develop uniforms"),
            size: std::mem::size_of::<DevelopUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rune-editor photo sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let curve_lut_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rune-editor curve lut 256x1 rgba16f"),
            size: wgpu::Extent3d {
                width: CURVE_LUT_SIZE as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let curve_lut_view = curve_lut_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let layer_mask_atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rune-editor layer mask atlas 1024x512 r8unorm"),
            size: wgpu::Extent3d {
                width: LAYER_MASK_ATLAS_WIDTH,
                height: LAYER_MASK_ATLAS_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let layer_mask_atlas_view =
            layer_mask_atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let profile_lut_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rune-editor profile lut 33x33x33 rgba16f"),
            size: wgpu::Extent3d {
                width: PROFILE_LUT_SIZE as u32,
                height: PROFILE_LUT_SIZE as u32,
                depth_or_array_layers: PROFILE_LUT_SIZE as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let profile_lut_view =
            profile_lut_texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            shader,
            bind_group_layout,
            pipeline_layout,
            pipelines: HashMap::new(),
            uniform_buffer,
            sampler,
            curve_lut_texture,
            curve_lut_view,
            uploaded_curves: None,
            layer_mask_atlas_texture,
            layer_mask_atlas_view,
            uploaded_layer_masks: None,
            profile_lut_texture,
            profile_lut_view,
            uploaded_profile_lut: None,
            photo_texture: None,
            bind_group: None,
            uploaded_generation: 0,
            uploaded_size: [1, 1],
        }
    }

    pub fn uniform_buffer(&self) -> &wgpu::Buffer {
        &self.uniform_buffer
    }

    fn write_curve_lut_f16(&self, queue: &wgpu::Queue, lut_f16: &[[half::f16; 4]; CURVE_LUT_SIZE]) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.curve_lut_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(lut_f16),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((CURVE_LUT_SIZE as u32) * 8), // 256 * 4 * 2 = 2048 bytes
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: CURVE_LUT_SIZE as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Uploads the baked `256x1` `Rgba16Float` tone curve LUT texture only when `recipe.curves`
    /// has changed since the last upload.
    pub fn ensure_curve_lut_uploaded(&mut self, queue: &wgpu::Queue, recipe: &DevelopRecipe) {
        if self.uploaded_curves.as_ref() == Some(&recipe.curves) {
            return;
        }
        let lut_f16 = recipe.curves.bake_curve_lut_f16();
        self.write_curve_lut_f16(queue, &lut_f16);
        self.uploaded_curves = Some(recipe.curves.clone());
    }

    fn write_layer_mask_atlas(&self, queue: &wgpu::Queue, atlas: &LayerMaskAtlas) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.layer_mask_atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas.width),
                rows_per_image: Some(atlas.height),
            },
            wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Bakes and uploads the `1024×512` `R8Unorm` layer mask atlas ONLY when layer masks
    /// have changed since the last upload (zero re-bake or allocation on 120Hz local slider drags).
    pub fn ensure_layer_masks_uploaded(&mut self, queue: &wgpu::Queue, recipe: &DevelopRecipe) {
        let slice = &recipe.layers[..recipe.layers.len().min(MAX_ADJUSTMENT_LAYERS)];
        let unchanged = self.uploaded_layer_masks.as_ref().is_some_and(|cached| {
            cached.len() == slice.len()
                && cached
                    .iter()
                    .zip(slice.iter())
                    .all(|(c, l)| c.as_slice() == l.masks.as_slice())
        });
        if unchanged {
            return;
        }
        let atlas = bake_layer_mask_atlas(slice);
        self.write_layer_mask_atlas(queue, &atlas);
        self.uploaded_layer_masks = Some(slice.iter().map(|l| l.masks.clone()).collect());
    }

    fn write_profile_lut_3d(&self, queue: &wgpu::Queue, lut: &ProfileLut3D) {
        let size = PROFILE_LUT_SIZE as u32;
        let rgba16f = lut.to_rgba16f();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.profile_lut_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&rgba16f),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 8), // 33 * 4 * 2 bytes per row
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: size,
            },
        );
    }

    /// Uploads the `33×33×33` `Rgba16Float` camera profile / film simulation 3D LUT texture
    /// ONLY when `recipe.camera_profile` or its resolved user profile LUT instance changes
    /// (zero re-bake or upload on 120Hz slider drags).
    pub fn ensure_profile_lut_uploaded(&mut self, queue: &wgpu::Queue, recipe: &DevelopRecipe) {
        self.ensure_profile_lut_uploaded_with_dir(queue, recipe, None);
    }

    /// Variant of [`Self::ensure_profile_lut_uploaded`] resolving `"user:<slug>"` camera profiles
    /// from an explicit `profiles_dir` (or `~/.config/rune-darkroom/profiles` when `None`).
    pub fn ensure_profile_lut_uploaded_with_dir(
        &mut self,
        queue: &wgpu::Queue,
        recipe: &DevelopRecipe,
        profiles_dir: Option<&Path>,
    ) {
        let canonical = canonical_profile_id(&recipe.camera_profile);
        let lut = resolve_profile_lut(&canonical, profiles_dir);
        let unchanged =
            self.uploaded_profile_lut
                .as_ref()
                .is_some_and(|(cached_id, cached_lut)| {
                    cached_id == &canonical
                        && match (cached_lut, &lut) {
                            (None, None) => true,
                            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                            _ => false,
                        }
                });
        if unchanged {
            return;
        }
        if let Some(lut_ref) = lut.as_deref() {
            self.write_profile_lut_3d(queue, lut_ref);
        } else {
            let identity = ProfileLut3D::identity(PROFILE_LUT_SIZE);
            self.write_profile_lut_3d(queue, &identity);
        }
        self.uploaded_profile_lut = Some((canonical, lut));
    }

    /// Uploads the scene-referred `Rgba16Float` image texture only when `generation` changes.
    /// Also initializes the tone curve LUT, layer mask atlas, and 3D profile LUT on first upload.
    pub fn ensure_image_uploaded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        generation: u64,
        image: &LinearImage,
    ) {
        if self.uploaded_curves.is_none() {
            let identity = ToneCurves::identity_lut_f16();
            self.write_curve_lut_f16(queue, &identity);
            self.uploaded_curves = Some(ToneCurves::default());
        }
        if self.uploaded_layer_masks.is_none() {
            let empty = LayerMaskAtlas::empty();
            self.write_layer_mask_atlas(queue, &empty);
            self.uploaded_layer_masks = Some(Vec::new());
        }
        if self.uploaded_profile_lut.is_none() {
            let identity = ProfileLut3D::identity(PROFILE_LUT_SIZE);
            self.write_profile_lut_3d(queue, &identity);
            self.uploaded_profile_lut = Some(("auto".to_owned(), None));
        }

        if self.uploaded_generation == generation && self.bind_group.is_some() {
            return;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rune-editor photo rgba16f"),
            size: wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            image.byte_slice(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 8), // 4 channels * 2 bytes (f16)
                rows_per_image: Some(image.height),
            },
            wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rune-editor develop bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.curve_lut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.layer_mask_atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&self.profile_lut_view),
                },
            ],
        });

        self.photo_texture = Some(texture);
        self.bind_group = Some(bind_group);
        self.uploaded_generation = generation;
        self.uploaded_size = [image.width, image.height];
    }

    pub fn image_size(&self) -> [u32; 2] {
        self.uploaded_size
    }

    /// Ensures a render pipeline exists for `target_format` and draws into `pass`.
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        pass: &mut wgpu::RenderPass<'_>,
    ) {
        let Some(bind_group) = self.bind_group.as_ref() else {
            return;
        };

        let pipeline = self.pipelines.entry(target_format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rune-editor develop pipeline"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        });

        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..6, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_editor_core::{
        BlackAndWhiteParams, ChannelLevels, ClarityMethod, ColorEditorSlice, ColorWheel,
        CompositionGrid, FilmCurve, GeometryParams, GrainType, HslBand, LensParams, LevelsParams,
        SkinToneParams, ToneCurves, VignetteMethod, evaluate_pixel_linear,
    };

    #[test]
    fn develop_uniforms_alignment_and_packing() {
        let size = std::mem::size_of::<DevelopUniforms>();
        assert_eq!(size, 2208);
        assert_eq!(size % 16, 0);

        let mut recipe = DevelopRecipe {
            camera_profile: "pro_standard".to_owned(),
            profile_strength: 0.75,
            film_curve: FilmCurve::FilmExtraShadow,
            shadows_wheel: ColorWheel::new(-0.2, -0.3, -0.1),
            midtones_wheel: ColorWheel::new(0.1, 0.05, 0.02),
            highlights_wheel: ColorWheel::new(0.3, 0.2, 0.04),
            master_wheel: ColorWheel::new(0.15, -0.1, 0.08),
            clarity: 0.35,
            clarity_method: ClarityMethod::Punch,
            sharpen: 0.5,
            sharpen_radius: 2.2,
            sharpen_threshold: 0.25,
            sharpen_halo: 0.4,
            noise_reduction: 0.25,
            nr_color: 0.6,
            structure: 0.42,
            brightness: 0.30,
            dehaze: 0.28,
            grain: 0.2,
            grain_type: GrainType::SilverRich,
            grain_size: 0.65,
            levels: LevelsParams {
                in_black: 0.05,
                in_mid: 0.15,
                in_white: 0.92,
                out_black: 0.01,
                out_white: 0.98,
                red: ChannelLevels {
                    in_black: 0.03,
                    in_mid: 0.1,
                    in_white: 0.95,
                    out_black: 0.02,
                    out_white: 0.97,
                },
                ..LevelsParams::default()
            },
            bw: BlackAndWhiteParams {
                enabled: true,
                red: 0.4,
                yellow: 0.2,
                green: -0.1,
                cyan: -0.3,
                blue: -0.6,
                magenta: 0.1,
                split_shadow_hue: 220.0,
                split_shadow_sat: 0.35,
                split_highlight_hue: 40.0,
                split_highlight_sat: 0.25,
            },
            geometry: GeometryParams {
                crop: [0.1, 0.2, 0.8, 0.7],
                straighten_deg: 5.0,
                rotate_90: 1,
                flip_h: true,
                flip_v: false,
                grid: CompositionGrid::GoldenRatio,
                ..GeometryParams::default()
            },
            ..DevelopRecipe::default()
        };
        recipe.hsl[5] = HslBand::new(0.15, -0.4, -0.2);

        let camera = ViewportCamera {
            exposure_warning: true,
            focus_mask: true,
            crop_mode: true,
            loupe_enabled: true,
            loupe_uv: [0.35, 0.65],
            loupe_zoom: 3.0,
            loupe_radius_px: 112.0,
            ..ViewportCamera::default()
        };
        let u = DevelopUniforms::build(
            [0.0, 0.0, 1280.0, 800.0],
            [1280, 800],
            [1600, 1066],
            &camera,
            &recipe,
            true,
        );
        assert_eq!(u.shadows_wheel, [-0.2, -0.3, -0.1, 0.0]);
        assert_eq!(u.midtones_wheel, [0.1, 0.05, 0.02, 0.0]);
        assert_eq!(u.highlights_wheel, [0.3, 0.2, 0.04, 0.0]);
        assert_eq!(u.master_wheel, [0.15, -0.1, 0.08, 0.0]);
        assert_eq!(u.hsl[5], [0.15, -0.4, -0.2, 0.0]);
        assert_eq!(u.detail, [0.35, 0.5, 0.25, 0.42]);
        assert_eq!(u.detail_ext, [2.2, 0.25, 0.4, 0.6]);
        assert_eq!(u.style_modes, [1.0, 1.0, 0.65, 0.0]);
        assert_eq!(u.geo_keystone, [0.0, 0.0, 0.0, 0.8]);
        assert_eq!(u.viewport_extra, [-1.0, 1.0, 0.0, 0.0]);
        assert_eq!(u.loupe_params, [0.35, 0.65, 3.0, 112.0]);
        assert_eq!(u.atmosphere, [0.30, 0.28, 0.01, 0.98]);
        assert_eq!(u.levels, [0.05, 0.15, 0.92, 1.0]);
        assert_eq!(u.levels_r_in, [0.03, 0.1, 0.95, 1.0]);
        assert_eq!(u.levels_g_in[3], 0.0);
        assert_eq!(u.levels_rgb_out[0..2], [0.02, 0.97]);
        assert_eq!(u.levels_b_out_curve[2..4], [1.0, 1.0]);
        assert_eq!(u.bw_channels[0], [1.0, 0.4, 0.2, -0.1]);
        assert_eq!(u.bw_channels[1], [-0.3, -0.6, 0.1, 0.0]);
        assert_eq!(u.bw_split, [220.0, 0.35, 40.0, 0.25]);
        assert_eq!(u.geo_crop, [0.1, 0.2, 0.8, 0.7]);
        assert!((u.geo_xform[0] - 5.0_f32.to_radians()).abs() < 1e-6);
        assert_eq!(u.geo_xform[1..4], [1.0, 1.0, 0.0]);
        assert_eq!(u.overlay_flags, [1.0, 1.0, 1.0, 2.0]);
        assert_eq!(u.profile_params, [1.0, 0.75, 0.0, 0.0]);

        // Out-of-disk / NaN wheels must be clamped safely in DevelopUniforms::build.
        let wild = DevelopRecipe {
            shadows_wheel: ColorWheel::new(3.0, 4.0, 5.0),
            midtones_wheel: ColorWheel::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY),
            master_wheel: ColorWheel::new(3.0, 4.0, f32::NAN),
            ..DevelopRecipe::default()
        };
        let uw = DevelopUniforms::build(
            [0.0, 0.0, 100.0, 100.0],
            [100, 100],
            [100, 100],
            &ViewportCamera::default(),
            &wild,
            false,
        );
        let len = uw.shadows_wheel[0].hypot(uw.shadows_wheel[1]);
        assert!((len - 1.0).abs() < 1e-5 && uw.shadows_wheel[2] == 1.0);
        assert_eq!(uw.midtones_wheel, [0.0, 0.0, 0.0, 0.0]);
        let mlen = uw.master_wheel[0].hypot(uw.master_wheel[1]);
        assert!((mlen - 1.0).abs() < 1e-5 && uw.master_wheel[2] == 0.0);
    }

    fn block_on_simple<F: std::future::Future>(fut: F) -> F::Output {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut fut = pin!(fut);
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(val) => return val,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn render_offscreen_rgba32f(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        gpu: &mut DevelopGpuResources,
        camera: &ViewportCamera,
        recipe: &DevelopRecipe,
        target_w: u32,
        target_h: u32,
    ) -> Vec<[f32; 4]> {
        render_offscreen_rgba32f_with_profiles_dir(
            device, queue, gpu, camera, recipe, target_w, target_h, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_offscreen_rgba32f_with_profiles_dir(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        gpu: &mut DevelopGpuResources,
        camera: &ViewportCamera,
        recipe: &DevelopRecipe,
        target_w: u32,
        target_h: u32,
        profiles_dir: Option<&Path>,
    ) -> Vec<[f32; 4]> {
        gpu.ensure_curve_lut_uploaded(queue, recipe);
        gpu.ensure_layer_masks_uploaded(queue, recipe);
        gpu.ensure_profile_lut_uploaded_with_dir(queue, recipe, profiles_dir);
        let target_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("parity target"),
            size: wgpu::Extent3d {
                width: target_w,
                height: target_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let uniforms = DevelopUniforms::build_with_profiles_dir(
            [0.0, 0.0, target_w as f32, target_h as f32],
            [target_w, target_h],
            gpu.image_size(),
            camera,
            recipe,
            false,
            profiles_dir,
        );
        queue.write_buffer(gpu.uniform_buffer(), 0, bytemuck::bytes_of(&uniforms));

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parity readback"),
            size: (target_w * target_h * 16) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("parity pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            gpu.draw(device, wgpu::TextureFormat::Rgba32Float, &mut pass);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target_w * 16),
                    rows_per_image: Some(target_h),
                },
            },
            wgpu::Extent3d {
                width: target_w,
                height: target_h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ready_flag = ready.clone();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            res.expect("readback map should succeed");
            ready_flag.store(true, std::sync::atomic::Ordering::Release);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        assert!(ready.load(std::sync::atomic::Ordering::Acquire));

        let data = slice.get_mapped_range().expect("mapped range");
        let pixels: Vec<[f32; 4]> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        readback.unmap();
        pixels
    }

    #[test]
    fn gpu_shader_compiles_and_matches_cpu_color_science() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = block_on_simple(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            ..Default::default()
        })) else {
            return;
        };
        let Ok((device, queue)) =
            block_on_simple(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            return;
        };

        let mut gpu = DevelopGpuResources::new(&device);

        // 16x16 uniform color test image for exact per-pixel color science parity.
        let src_rgb = [0.22_f32, 0.14_f32, 0.42_f32];
        let mut rgba16f = Vec::with_capacity(16 * 16 * 4);
        for _ in 0..(16 * 16) {
            rgba16f.push(half::f16::from_f32(src_rgb[0]));
            rgba16f.push(half::f16::from_f32(src_rgb[1]));
            rgba16f.push(half::f16::from_f32(src_rgb[2]));
            rgba16f.push(half::f16::from_f32(1.0));
        }
        let img = LinearImage {
            width: 16,
            height: 16,
            rgba16f,
            histogram_proxy: vec![src_rgb],
            display_name: "parity_test".to_owned(),
            source_path: None,
            format_label: "TEST".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        gpu.ensure_image_uploaded(&device, &queue, 1, &img);

        // Full color stack including Brightness, Levels, Dehaze, Curves, Wheels, HSL.
        let mut recipe = DevelopRecipe {
            exposure: 0.35,
            brightness: 0.25,
            contrast: 0.22,
            highlights: -0.25,
            shadows: 0.20,
            whites: 0.08,
            blacks: -0.06,
            temperature: 0.18,
            tint: -0.08,
            dehaze: 0.32,
            levels: LevelsParams {
                in_black: 0.04,
                in_mid: 0.18,
                in_white: 0.92,
                out_black: 0.02,
                out_white: 0.98,
                red: ChannelLevels {
                    in_black: 0.02,
                    in_mid: 0.12,
                    in_white: 0.96,
                    out_black: 0.01,
                    out_white: 0.99,
                },
                green: ChannelLevels {
                    in_mid: -0.08,
                    ..ChannelLevels::default()
                },
                blue: ChannelLevels {
                    in_black: 0.03,
                    out_white: 0.95,
                    ..ChannelLevels::default()
                },
            },
            vibrance: 0.25,
            saturation: 0.10,
            shadows_wheel: ColorWheel::new(-0.18, -0.24, -0.05),
            midtones_wheel: ColorWheel::new(0.12, 0.06, 0.04),
            highlights_wheel: ColorWheel::new(0.25, 0.15, 0.02),
            master_wheel: ColorWheel::new(0.10, -0.08, 0.05),
            curves: ToneCurves {
                rgb: vec![[0.0, 0.01], [0.5, 0.54], [1.0, 0.99]],
                luma: vec![[0.0, 0.02], [0.25, 0.20], [0.75, 0.82], [1.0, 0.98]],
                red: vec![[0.0, 0.0], [0.5, 0.54], [1.0, 1.0]],
                blue: vec![[0.0, 0.02], [0.5, 0.47], [1.0, 0.96]],
                ..ToneCurves::default()
            },
            ..DevelopRecipe::default()
        };
        recipe.hsl[5] = HslBand::new(-0.12, 0.18, -0.10);
        recipe.hsl[6] = HslBand::new(0.08, -0.15, 0.08);
        recipe.advanced_color[0] = ColorEditorSlice {
            enabled: true,
            target_hue: 255.0,
            target_sat: 0.45,
            hue_range: 45.0,
            sat_range: 0.50,
            smoothness: 0.40,
            hue_shift: 0.20,
            saturation: 0.15,
            lightness: -0.10,
            uniformity: 0.35,
        };
        recipe.skin_tone = SkinToneParams {
            enabled: true,
            target_hue: 255.0,
            target_sat: 0.45,
            target_lightness: 0.60,
            smoothness: 0.50,
            uniformity_hue: 0.30,
            uniformity_sat: 0.25,
            uniformity_lightness: 0.20,
            amount_hue: -0.10,
            amount_sat: 0.12,
            amount_lightness: 0.08,
        };

        let camera = ViewportCamera {
            zoom: 4.0,
            ..ViewportCamera::default()
        };
        for curve in FilmCurve::ALL {
            recipe.film_curve = curve;
            let pixels =
                render_offscreen_rgba32f(&device, &queue, &mut gpu, &camera, &recipe, 64, 64);
            let center_gpu = pixels[32 * 64 + 32];
            let cpu = evaluate_pixel_linear(src_rgb, &recipe);
            for ch in 0..3 {
                assert!(
                    (center_gpu[ch] - cpu[ch]).abs() < 2e-3,
                    "Color GPU/CPU parity drift ({curve:?}) ch {ch}: gpu={} vs cpu={}",
                    center_gpu[ch],
                    cpu[ch]
                );
            }
        }
        recipe.film_curve = FilmCurve::FilmStandard;

        // Same full stack + 6-Channel B&W Mixer + Split Toning enabled.
        recipe.bw = BlackAndWhiteParams {
            enabled: true,
            red: 0.35,
            yellow: 0.15,
            green: -0.20,
            cyan: -0.35,
            blue: -0.65,
            magenta: 0.25,
            split_shadow_hue: 220.0,
            split_shadow_sat: 0.45,
            split_highlight_hue: 38.0,
            split_highlight_sat: 0.35,
        };
        let bw_pixels =
            render_offscreen_rgba32f(&device, &queue, &mut gpu, &camera, &recipe, 64, 64);
        let bw_gpu = bw_pixels[32 * 64 + 32];
        let bw_cpu = evaluate_pixel_linear(src_rgb, &recipe);
        for ch in 0..3 {
            assert!(
                (bw_gpu[ch] - bw_cpu[ch]).abs() < 2e-3,
                "B&W GPU/CPU parity drift ch {ch}: gpu={} vs cpu={}",
                bw_gpu[ch],
                bw_cpu[ch]
            );
        }

        // Negative dehaze (-0.45) GPU-vs-CPU parity check.
        let neg_dehaze_recipe = DevelopRecipe {
            dehaze: -0.45,
            ..DevelopRecipe::default()
        };
        let neg_px = render_offscreen_rgba32f(
            &device,
            &queue,
            &mut gpu,
            &camera,
            &neg_dehaze_recipe,
            64,
            64,
        );
        let neg_gpu = neg_px[32 * 64 + 32];
        let neg_cpu = evaluate_pixel_linear(src_rgb, &neg_dehaze_recipe);
        for ch in 0..3 {
            assert!(
                (neg_gpu[ch] - neg_cpu[ch]).abs() < 2e-3,
                "Negative dehaze GPU/CPU parity drift ch {ch}: gpu={} vs cpu={}",
                neg_gpu[ch],
                neg_cpu[ch]
            );
        }

        // Exposure warning overlay: overexposed highlight turns red, crushed shadow turns blue.
        let clip_recipe = DevelopRecipe {
            exposure: 4.0,
            ..DevelopRecipe::default()
        };
        let warn_cam = ViewportCamera {
            zoom: 4.0,
            exposure_warning: true,
            ..ViewportCamera::default()
        };
        let warn_px =
            render_offscreen_rgba32f(&device, &queue, &mut gpu, &warn_cam, &clip_recipe, 64, 64);
        let c = warn_px[32 * 64 + 32];
        assert!(
            c[0] > 0.75 && c[1] < 0.35 && c[2] < 0.35,
            "Expected red highlight warning, got {c:?}"
        );

        let shadow_clip_recipe = DevelopRecipe {
            levels: LevelsParams {
                in_black: 0.90,
                in_mid: 0.0,
                in_white: 1.0,
                out_black: 0.0,
                out_white: 1.0,
                ..LevelsParams::default()
            },
            ..DevelopRecipe::default()
        };
        let shadow_warn_px = render_offscreen_rgba32f(
            &device,
            &queue,
            &mut gpu,
            &warn_cam,
            &shadow_clip_recipe,
            64,
            64,
        );
        let sc = shadow_warn_px[32 * 64 + 32];
        assert!(
            sc[2] > 0.65 && sc[0] < 0.15 && sc[1] < 0.25,
            "Expected blue shadow warning, got {sc:?}"
        );

        // Focus Mask & Geometry (90 deg CW + Crop) test on a 4-quadrant high-contrast image.
        let mut quad_rgba = Vec::with_capacity(16 * 16 * 4);
        for y in 0..16 {
            for x in 0..16 {
                // Top-left = Red, Top-right = Green, Bottom-left = Blue, Bottom-right = White
                let rgb = match (x < 8, y < 8) {
                    (true, true) => [0.6, 0.02, 0.02],
                    (false, true) => [0.02, 0.6, 0.02],
                    (true, false) => [0.02, 0.02, 0.6],
                    (false, false) => [0.6, 0.6, 0.6],
                };
                for ch in rgb {
                    quad_rgba.push(half::f16::from_f32(ch));
                }
                quad_rgba.push(half::f16::from_f32(1.0));
            }
        }
        let quad_img = LinearImage {
            width: 16,
            height: 16,
            rgba16f: quad_rgba,
            histogram_proxy: vec![[0.2, 0.2, 0.2]],
            display_name: "quad_test".to_owned(),
            source_path: None,
            format_label: "TEST".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        gpu.ensure_image_uploaded(&device, &queue, 2, &quad_img);

        // Rotate 90 CW moves source top-left (Red) to oriented top-right ([0.5..1, 0..0.5]).
        // Cropping to oriented top-right must yield pure Red at viewport center!
        let rot_recipe = DevelopRecipe {
            geometry: GeometryParams {
                rotate_90: 1,
                crop: [0.55, 0.05, 0.40, 0.40],
                ..GeometryParams::default()
            },
            ..DevelopRecipe::default()
        };
        let rot_px =
            render_offscreen_rgba32f(&device, &queue, &mut gpu, &camera, &rot_recipe, 64, 64);
        let rc = rot_px[32 * 64 + 32];
        assert!(
            rc[0] > 0.5 && rc[1] < 0.05 && rc[2] < 0.05,
            "90 CW + top-right crop should sample source top-left Red, got {rc:?}"
        );

        // Focus Mask turns center seam (sharp step edge) bright green.
        let focus_cam = ViewportCamera {
            zoom: 4.0,
            focus_mask: true,
            ..ViewportCamera::default()
        };
        let focus_px = render_offscreen_rgba32f(
            &device,
            &queue,
            &mut gpu,
            &focus_cam,
            &DevelopRecipe::default(),
            64,
            64,
        );
        let edge_px = focus_px[32 * 64 + 32];
        assert!(
            edge_px[1] > edge_px[0] && edge_px[1] > 0.4,
            "Focus mask should highlight sharp step edge green, got {edge_px:?}"
        );

        // Full-resolution 1:1 GPU vs CPU spatial detail (NR + Clarity + Structure + Sharpen) + Vignette + Grain parity.
        // 40x40 textured linear scene rendered into a 96x96 target (pad = 28px -> exact 1:1 40x40 interior pixels;
        // 96 * 16 = 1536 bytes/row respects wgpu's 256-byte COPY_BYTES_PER_ROW_ALIGNMENT).
        let mut tex_rgba = Vec::with_capacity(40 * 40 * 4);
        for y in 0..40u32 {
            for x in 0..40u32 {
                let base = if x < 20 { 0.14 } else { 0.34 };
                let wave = ((x as f32 * 0.9).sin() * 0.04) + ((y as f32 * 1.1).cos() * 0.03);
                let r = (base + wave + 0.02).max(0.01);
                let g = (base + wave * 0.6).max(0.01);
                let b = (base - wave * 0.5 + 0.04).max(0.01);
                tex_rgba.push(half::f16::from_f32(r));
                tex_rgba.push(half::f16::from_f32(g));
                tex_rgba.push(half::f16::from_f32(b));
                tex_rgba.push(half::f16::from_f32(1.0));
            }
        }
        let tex_img = LinearImage {
            width: 40,
            height: 40,
            rgba16f: tex_rgba,
            histogram_proxy: vec![[0.2, 0.2, 0.2]],
            display_name: "spatial_parity".to_owned(),
            source_path: None,
            format_label: "TEST".to_owned(),
            exif: None,
            camera_calibration: None,
        };
        gpu.ensure_image_uploaded(&device, &queue, 3, &tex_img);

        let fit_cam = ViewportCamera::default();
        let stage_cases: [(&str, DevelopRecipe); 18] = [
            (
                "noise_reduction",
                DevelopRecipe {
                    noise_reduction: 0.35,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "nr_color_only",
                DevelopRecipe {
                    nr_color: 0.70,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "clarity_natural",
                DevelopRecipe {
                    clarity: 0.45,
                    clarity_method: ClarityMethod::Natural,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "clarity_punch",
                DevelopRecipe {
                    clarity: 0.45,
                    clarity_method: ClarityMethod::Punch,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "clarity_neutral",
                DevelopRecipe {
                    clarity: 0.45,
                    clarity_method: ClarityMethod::Neutral,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "clarity_classic",
                DevelopRecipe {
                    clarity: 0.45,
                    clarity_method: ClarityMethod::Classic,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "structure",
                DevelopRecipe {
                    structure: 0.40,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "sharpen_default",
                DevelopRecipe {
                    sharpen: 0.50,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "sharpen_extended",
                DevelopRecipe {
                    sharpen: 0.65,
                    sharpen_radius: 2.2,
                    sharpen_threshold: 0.20,
                    sharpen_halo: 0.55,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "vignette",
                DevelopRecipe {
                    exposure: 0.20,
                    vignette: -0.35,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "vignette_circular_crop",
                DevelopRecipe {
                    vignette: -0.45,
                    vignette_method: VignetteMethod::CircularOnCrop,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "vignette_circular_full",
                DevelopRecipe {
                    vignette: -0.45,
                    vignette_method: VignetteMethod::Circular,
                    ..DevelopRecipe::default()
                },
            ),
            (
                "lens_light_falloff_and_fringing",
                DevelopRecipe {
                    lens: LensParams {
                        purple_fringing: 0.75,
                        light_falloff: 0.65,
                        ..LensParams::default()
                    },
                    ..DevelopRecipe::default()
                },
            ),
            (
                "lens_distortion_and_ca",
                DevelopRecipe {
                    lens: LensParams {
                        distortion: 0.35,
                        chromatic_aberration: 0.50,
                        ..LensParams::default()
                    },
                    ..DevelopRecipe::default()
                },
            ),
            (
                "keystone_perspective",
                DevelopRecipe {
                    geometry: GeometryParams {
                        keystone_vertical: 0.35,
                        keystone_horizontal: -0.20,
                        keystone_aspect: 0.15,
                        keystone_amount: 0.80,
                        ..GeometryParams::default()
                    },
                    ..DevelopRecipe::default()
                },
            ),
            (
                "straighten_keystone_and_neg_distortion",
                DevelopRecipe {
                    geometry: GeometryParams {
                        straighten_deg: 12.0,
                        keystone_vertical: 0.30,
                        keystone_amount: 0.80,
                        ..GeometryParams::default()
                    },
                    lens: LensParams {
                        distortion: -0.85,
                        ..LensParams::default()
                    },
                    ..DevelopRecipe::default()
                },
            ),
            (
                "advanced_color_and_skin_tone",
                DevelopRecipe {
                    advanced_color: [
                        ColorEditorSlice {
                            enabled: true,
                            target_hue: 215.0,
                            target_sat: 0.25,
                            hue_range: 60.0,
                            sat_range: 0.50,
                            smoothness: 0.45,
                            hue_shift: 0.25,
                            saturation: 0.20,
                            lightness: -0.10,
                            uniformity: 0.40,
                        },
                        ColorEditorSlice::default(),
                        ColorEditorSlice::default(),
                        ColorEditorSlice::default(),
                    ],
                    skin_tone: SkinToneParams {
                        enabled: true,
                        target_hue: 35.0,
                        target_sat: 0.20,
                        target_lightness: 0.55,
                        smoothness: 0.50,
                        uniformity_hue: 0.40,
                        uniformity_sat: 0.30,
                        uniformity_lightness: 0.25,
                        amount_hue: 0.10,
                        amount_sat: 0.15,
                        amount_lightness: 0.05,
                    },
                    ..DevelopRecipe::default()
                },
            ),
            (
                "combined_spatial_and_vignette",
                DevelopRecipe {
                    exposure: 0.20,
                    film_curve: FilmCurve::FilmExtraShadow,
                    noise_reduction: 0.35,
                    nr_color: 0.50,
                    clarity: 0.45,
                    clarity_method: ClarityMethod::Punch,
                    structure: 0.40,
                    sharpen: 0.50,
                    sharpen_radius: 1.8,
                    sharpen_threshold: 0.15,
                    sharpen_halo: 0.40,
                    vignette: -0.35,
                    ..DevelopRecipe::default()
                },
            ),
        ];

        for (label, spatial_recipe) in stage_cases {
            let gpu_px = render_offscreen_rgba32f(
                &device,
                &queue,
                &mut gpu,
                &fit_cam,
                &spatial_recipe,
                96,
                96,
            );
            let (cw, ch, cpu_srgb) = rune_editor_decode::export::render_developed_srgb_f32(
                &tex_img,
                &spatial_recipe,
                &rune_editor_decode::ExportOptions::default(),
            )
            .expect("render cpu spatial parity");
            assert_eq!((cw, ch), (40, 40));

            let mut max_diff = 0.0_f32;
            let mut worst = (0u32, 0u32, 0usize, 0.0_f32, 0.0_f32);
            for y in 2..38u32 {
                for x in 2..38u32 {
                    let gp = gpu_px[((28 + y) * 96 + (28 + x)) as usize];
                    let cp = cpu_srgb[(y * cw + x) as usize];
                    for ch in 0..3 {
                        let gpu_srgb = rune_editor_core::linear_to_srgb(gp[ch]);
                        let diff = (gpu_srgb - cp[ch]).abs();
                        if diff > max_diff {
                            max_diff = diff;
                            worst = (x, y, ch, gpu_srgb, cp[ch]);
                        }
                    }
                }
            }
            assert!(
                max_diff < 2e-3,
                "GPU vs CPU parity drift for '{label}': max={max_diff} at ({},{}) ch={} gpu={} cpu={}",
                worst.0,
                worst.1,
                worst.2,
                worst.3,
                worst.4
            );
        }

        // Step 12 post-vignette film grain parity across all 6 Capture One GrainType profiles + grain_size.
        // Chaotic `fract()` in `hash12` can flip across an integer boundary on ~0.1% of coordinates due to
        // GPU vs CPU FMA rounding; verify >=99% exact (<2e-3) pixel parity across the frame.
        for grain_type in GrainType::ALL {
            let grain_recipe = DevelopRecipe {
                exposure: 1.0,
                vignette: -0.6,
                grain: 0.35,
                grain_type,
                grain_size: 0.60,
                ..DevelopRecipe::default()
            };
            let gpu_grain = render_offscreen_rgba32f(
                &device,
                &queue,
                &mut gpu,
                &fit_cam,
                &grain_recipe,
                96,
                96,
            );
            let (cw, _ch, cpu_grain) = rune_editor_decode::export::render_developed_srgb_f32(
                &tex_img,
                &grain_recipe,
                &rune_editor_decode::ExportOptions::default(),
            )
            .expect("render cpu grain parity");
            let mut total_px = 0usize;
            let mut exact_px = 0usize;
            for y in 2..38u32 {
                for x in 2..38u32 {
                    total_px += 1;
                    let gp = gpu_grain[((28 + y) * 96 + (28 + x)) as usize];
                    let cp = cpu_grain[(y * cw + x) as usize];
                    let px_ok = (0..3)
                        .all(|ch| (rune_editor_core::linear_to_srgb(gp[ch]) - cp[ch]).abs() < 2e-3);
                    if px_ok {
                        exact_px += 1;
                    }
                }
            }
            assert!(
                exact_px * 100 >= total_px * 99,
                "GPU vs CPU grain parity too low for {grain_type:?}: {exact_px}/{total_px} pixels within 2e-3"
            );
        }

        // Verify transient `view_color_range` desaturates out-of-range colors on GPU without altering target colors.
        let mut vr_recipe = DevelopRecipe::default();
        vr_recipe.advanced_color[0] = ColorEditorSlice {
            enabled: true,
            target_hue: 0.0, // Red
            target_sat: 0.85,
            hue_range: 25.0,
            sat_range: 0.45,
            smoothness: 0.2,
            ..ColorEditorSlice::default()
        };
        gpu.ensure_image_uploaded(&device, &queue, 4, &quad_img);
        let vr_cam = ViewportCamera {
            view_color_range: 0,
            ..ViewportCamera::default()
        };
        let vr_px =
            render_offscreen_rgba32f(&device, &queue, &mut gpu, &vr_cam, &vr_recipe, 96, 96);
        // Top-left quadrant (Red) stays colorful; top-right quadrant (Green) desaturates to grayscale.
        let red_sample = vr_px[36 * 96 + 36];
        let green_sample = vr_px[36 * 96 + 56];
        assert!(
            red_sample[0] > red_sample[1] + 0.2,
            "view_color_range should preserve selected red slice: {red_sample:?}"
        );
        assert!(
            (green_sample[0] - green_sample[1]).abs() < 0.02
                && (green_sample[1] - green_sample[2]).abs() < 0.02,
            "view_color_range should desaturate out-of-range green: {green_sample:?}"
        );

        // Verify 120Hz GPU Loupe overlay magnifies around loupe_uv, draws neutral rim,
        // and leaves pixels outside the loupe circle identical to loupe_enabled=false.
        let base_px = render_offscreen_rgba32f(
            &device,
            &queue,
            &mut gpu,
            &ViewportCamera::default(),
            &DevelopRecipe::default(),
            96,
            96,
        );
        let loupe_cam = ViewportCamera {
            loupe_enabled: true,
            loupe_uv: [0.55, 0.55],
            loupe_zoom: 3.0,
            loupe_radius_px: 24.0,
            ..ViewportCamera::default()
        };
        let loupe_px = render_offscreen_rgba32f(
            &device,
            &queue,
            &mut gpu,
            &loupe_cam,
            &DevelopRecipe::default(),
            96,
            96,
        );
        // Pixel well outside loupe radius (dist ~32px > 24 + 3.5) inside the photo frame must be bit-identical.
        let out_a = base_px[30 * 96 + 30];
        let out_b = loupe_px[30 * 96 + 30];
        for ch in 0..3 {
            assert!(
                (out_a[ch] - out_b[ch]).abs() < 1e-4,
                "pixel outside GPU loupe changed: base={out_a:?} loupe={out_b:?}"
            );
        }
        // Pixel on loupe rim (round(52.8 + 24.0)=77, 53) should reflect the bright neutral studio rim.
        let rim_px = loupe_px[53 * 96 + 77];
        assert!(
            rim_px[0] > 0.35 && (rim_px[0] - rim_px[2]).abs() < 0.12,
            "GPU loupe rim should be neutral and visible: {rim_px:?}"
        );
        // Center crosshair pixel (53, 53) must be finite and bright.
        let center_px = loupe_px[53 * 96 + 53];
        assert!(
            center_px.iter().all(|v| v.is_finite()) && center_px[0] > 0.4,
            "GPU loupe center crosshair should be bright & finite: {center_px:?}"
        );

        // Phase 4 GPU <-> CPU (< 2e-3) parity across Linear+Radial combined masks, Luma Range,
        // Brush mask, Local Adjustments (exposure/temp/contrast/sat/dehaze/clarity/sharpen), and Heal/Clone!
        use rune_editor_core::{
            AdjustmentLayer, BrushDab, HealMode, HealSpot, LayerMaskItem, LocalAdjustments,
            MaskCombineOp, MaskShape,
        };
        gpu.ensure_image_uploaded(&device, &queue, 5, &tex_img);

        let mut combo_layer = AdjustmentLayer::new("l1", "Linear+Radial");
        combo_layer.opacity = 0.85;
        combo_layer.masks.push(LayerMaskItem {
            enabled: true,
            op: MaskCombineOp::Add,
            inverted: false,
            shape: MaskShape::LinearGradient {
                start: [0.5, 0.0],
                end: [0.5, 0.85],
                feather: 0.4,
            },
        });
        combo_layer.masks.push(LayerMaskItem {
            enabled: true,
            op: MaskCombineOp::Subtract,
            inverted: false,
            shape: MaskShape::RadialGradient {
                center: [0.5, 0.35],
                radius: [0.22, 0.22],
                angle_deg: 0.0,
                feather: 0.35,
            },
        });
        combo_layer.adjustments = LocalAdjustments {
            exposure: 0.65,
            contrast: 0.2,
            temperature: 0.25,
            saturation: 0.15,
            dehaze: 0.18,
            ..LocalAdjustments::default()
        };

        let mut luma_layer = AdjustmentLayer::new("l2", "Highlights Luma");
        luma_layer.masks.push(LayerMaskItem {
            enabled: true,
            op: MaskCombineOp::Add,
            inverted: false,
            shape: MaskShape::LumaRange {
                low: 0.45,
                high: 1.0,
                low_falloff: 0.15,
                high_falloff: 0.0,
            },
        });
        luma_layer.adjustments = LocalAdjustments {
            highlights: -0.45,
            brightness: -0.15,
            ..LocalAdjustments::default()
        };

        let mut brush_layer = AdjustmentLayer::new("l3", "Brush Dodge");
        brush_layer.masks.push(LayerMaskItem {
            enabled: true,
            op: MaskCombineOp::Add,
            inverted: false,
            shape: MaskShape::BrushStrokes {
                strokes: vec![
                    BrushDab {
                        x: 0.35,
                        y: 0.65,
                        radius: 0.22,
                        hardness: 0.45,
                        opacity: 0.9,
                        erase: false,
                    },
                    BrushDab {
                        x: 0.35,
                        y: 0.65,
                        radius: 0.08,
                        hardness: 0.6,
                        opacity: 0.8,
                        erase: true,
                    },
                ],
            },
        });
        brush_layer.adjustments = LocalAdjustments {
            exposure: 0.55,
            clarity: 0.25,
            sharpen: 0.3,
            ..LocalAdjustments::default()
        };

        let phase4_cases: [(&str, DevelopRecipe); 5] = [
            (
                "linear_radial_combined_layer",
                DevelopRecipe {
                    layers: vec![combo_layer.clone()],
                    ..DevelopRecipe::default()
                },
            ),
            (
                "luma_range_layer",
                DevelopRecipe {
                    layers: vec![luma_layer],
                    ..DevelopRecipe::default()
                },
            ),
            (
                "brush_clarity_sharpen_layer",
                DevelopRecipe {
                    layers: vec![brush_layer],
                    ..DevelopRecipe::default()
                },
            ),
            (
                "spot_heal_and_clone",
                DevelopRecipe {
                    heal_spots: vec![
                        HealSpot {
                            id: "h1".to_owned(),
                            target: [0.30, 0.30],
                            source: [0.70, 0.30],
                            radius: 0.10,
                            feather: 0.4,
                            opacity: 1.0,
                            mode: HealMode::Heal,
                        },
                        HealSpot {
                            id: "c1".to_owned(),
                            target: [0.70, 0.70],
                            source: [0.25, 0.70],
                            radius: 0.10,
                            feather: 0.3,
                            opacity: 0.9,
                            mode: HealMode::Clone,
                        },
                    ],
                    ..DevelopRecipe::default()
                },
            ),
            (
                "film_negative_conversion",
                DevelopRecipe {
                    processing_mode: rune_editor_core::ProcessingMode::FilmNegative,
                    film_negative: rune_editor_core::FilmNegativeParams {
                        base_r: 0.88,
                        base_g: 0.54,
                        base_b: 0.33,
                        density: 0.25,
                        contrast: 0.18,
                        black_clip: 0.04,
                        white_clip: 0.96,
                    },
                    exposure: 0.2,
                    temperature: -0.1,
                    ..DevelopRecipe::default()
                },
            ),
        ];

        for (name, p4_recipe) in &phase4_cases {
            let gpu_p4 = render_offscreen_rgba32f(
                &device,
                &queue,
                &mut gpu,
                &ViewportCamera::default(),
                p4_recipe,
                96,
                96,
            );
            let (cw, ch, cpu_p4) = rune_editor_decode::export::render_developed_srgb_f32(
                &tex_img,
                p4_recipe,
                &rune_editor_decode::ExportOptions::default(),
            )
            .unwrap();
            assert_eq!((cw, ch), (40, 40));
            let mut max_diff = 0.0_f32;
            for y in 2..38u32 {
                for x in 2..38u32 {
                    let gp = gpu_p4[((28 + y) * 96 + (28 + x)) as usize];
                    let cp = cpu_p4[(y * cw + x) as usize];
                    for ch in 0..3 {
                        let d = (rune_editor_core::linear_to_srgb(gp[ch]) - cp[ch]).abs();
                        if d > max_diff {
                            max_diff = d;
                        }
                    }
                }
            }
            assert!(
                max_diff < 2e-3,
                "Phase 4 GPU vs CPU parity failed for '{name}': max sRGB diff {max_diff:.6} >= 2e-3"
            );
        }

        // Verify Mask Overlay Display Modes (1 = Red Rubylith, 2 = Grayscale) on GPU.
        let overlay_recipe = DevelopRecipe {
            layers: vec![combo_layer],
            ..DevelopRecipe::default()
        };
        let red_cam = ViewportCamera {
            mask_overlay_mode: 1,
            active_layer_index: 0,
            ..ViewportCamera::default()
        };
        let gray_cam = ViewportCamera {
            mask_overlay_mode: 2,
            active_layer_index: 0,
            ..ViewportCamera::default()
        };
        let red_px =
            render_offscreen_rgba32f(&device, &queue, &mut gpu, &red_cam, &overlay_recipe, 96, 96);
        let gray_px = render_offscreen_rgba32f(
            &device,
            &queue,
            &mut gpu,
            &gray_cam,
            &overlay_recipe,
            96,
            96,
        );
        // Top-left inside mask (28+4, 28+4) -> red tint in mode 1, bright white in mode 2.
        let top_red = red_px[32 * 96 + 32];
        let top_gray = gray_px[32 * 96 + 32];
        let bot_gray = gray_px[64 * 96 + 48];
        assert!(
            top_red[0] > top_red[1] + 0.25,
            "Red mask overlay should tint masked region red: {top_red:?}"
        );
        assert!(
            top_gray[0] > 0.85 && (top_gray[0] - top_gray[2]).abs() < 1e-3,
            "Grayscale mask overlay should render masked top pure white: {top_gray:?}"
        );
        assert!(
            bot_gray[0] < 0.05 && (bot_gray[0] - bot_gray[2]).abs() < 1e-3,
            "Grayscale mask overlay should render unmasked bottom pure black: {bot_gray:?}"
        );

        // Phase 1 Camera Profile 33^3 3D LUT GPU <-> CPU < 2e-3 parity across key profiles at 1.0 and 0.5 strength.
        for profile_id in [
            "pro_standard",
            "classic_chrome",
            "vivid_velvia",
            "acros_mono",
        ] {
            for strength in [1.0_f32, 0.5_f32] {
                let prof_recipe = DevelopRecipe {
                    camera_profile: profile_id.to_owned(),
                    profile_strength: strength,
                    exposure: 0.15,
                    ..DevelopRecipe::default()
                };
                let gpu_prof = render_offscreen_rgba32f(
                    &device,
                    &queue,
                    &mut gpu,
                    &ViewportCamera::default(),
                    &prof_recipe,
                    96,
                    96,
                );
                let (cw, ch, cpu_prof) = rune_editor_decode::export::render_developed_srgb_f32(
                    &tex_img,
                    &prof_recipe,
                    &rune_editor_decode::ExportOptions::default(),
                )
                .expect("render cpu profile parity");
                assert_eq!((cw, ch), (40, 40));

                let mut max_diff = 0.0_f32;
                let mut worst = (0u32, 0u32, 0usize, 0.0_f32, 0.0_f32);
                for y in 2..38u32 {
                    for x in 2..38u32 {
                        let gp = gpu_prof[((28 + y) * 96 + (28 + x)) as usize];
                        let cp = cpu_prof[(y * cw + x) as usize];
                        for ch in 0..3 {
                            let gpu_srgb = rune_editor_core::linear_to_srgb(gp[ch]);
                            let diff = (gpu_srgb - cp[ch]).abs();
                            if diff > max_diff {
                                max_diff = diff;
                                worst = (x, y, ch, gpu_srgb, cp[ch]);
                            }
                        }
                    }
                }
                assert!(
                    max_diff < 2e-3,
                    "Camera profile GPU vs CPU parity drift for '{profile_id}' (strength={strength}): max={max_diff:.6} at ({},{}) ch={} gpu={} cpu={}",
                    worst.0,
                    worst.1,
                    worst.2,
                    worst.3,
                    worst.4
                );
            }
        }

        // Phase 2 External User Camera Profiles (.dcp, .cube, .icc matrix-shaper, .icc A2B0 mft2)
        // GPU <-> CPU < 2e-3 parity + on-disk overwrite cache invalidation test.
        use rune_editor_core::{
            import_user_profile_in_dir, import_user_profile_in_dir_with_overwrite,
        };
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let store_dir = std::env::temp_dir().join(format!(
            "rune_engine_user_profiles_{}_{nanos}",
            std::process::id()
        ));
        let src_dir = std::env::temp_dir().join(format!(
            "rune_engine_user_profiles_src_{}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&store_dir).expect("mkdir store");
        std::fs::create_dir_all(&src_dir).expect("mkdir src");

        // 1) Synthetic .cube (17^3, non-default domain)
        let build_cube = |title: &str, warm_r: f32, cool_b: f32| -> Vec<u8> {
            let mut s = format!(
                "# GPU parity .cube\nTITLE \"{title}\"\nLUT_3D_SIZE 17\nDOMAIN_MIN 0.02 0.02 0.02\nDOMAIN_MAX 0.98 0.98 0.98\n"
            );
            for b in 0..17 {
                let fb = b as f32 / 16.0;
                for g in 0..17 {
                    let fg = g as f32 / 16.0;
                    for r in 0..17 {
                        let fr = r as f32 / 16.0;
                        s.push_str(&format!(
                            "{:.6} {:.6} {:.6}\n",
                            fr.powf(warm_r),
                            fg,
                            fb.powf(cool_b)
                        ));
                    }
                }
            }
            s.into_bytes()
        };
        let cube_path = src_dir.join("cinema.cube");
        std::fs::write(&cube_path, build_cube("Cinema Warm", 0.82, 1.18)).unwrap();
        let cube_desc = import_user_profile_in_dir(&store_dir, &cube_path, None).unwrap();

        // 2) Synthetic .dcp (LE with HueSatMap + LookTable + ProfileToneCurve)
        let dcp_bytes = {
            let mut buf = b"II".to_vec();
            buf.extend_from_slice(&0x4352u16.to_le_bytes());
            buf.extend_from_slice(&8u32.to_le_bytes());
            let model = b"SONY ILCE-7RM5\0";
            let name = b"Engine Test DCP\0";
            let hsm_dims = [6u32, 2, 2];
            let mut hsm_data = Vec::new();
            for _v in 0..2 {
                for h in 0..6 {
                    for s in 0..2 {
                        hsm_data.extend_from_slice(&[
                            if h == 0 && s == 1 { 7.0_f32 } else { 0.0 },
                            if s == 1 { 1.14_f32 } else { 1.0 },
                            1.0_f32,
                        ]);
                    }
                }
            }
            let curve: [f32; 8] = [0.0, 0.0, 0.25, 0.21, 0.75, 0.81, 1.0, 1.0];
            let entry_count = 5u16;
            let data_start = 8u32 + 2 + (entry_count as u32) * 12 + 4;
            let mut pay = Vec::new();
            let mut add = |src: &[u8]| -> u32 {
                let off = data_start + pay.len() as u32;
                pay.extend_from_slice(src);
                while !pay.len().is_multiple_of(4) {
                    pay.push(0);
                }
                off
            };
            let m_off = add(model);
            let n_off = add(name);
            let hd_off = add(bytemuck::cast_slice(&hsm_dims));
            let h_off = add(bytemuck::cast_slice(&hsm_data));
            let c_off = add(bytemuck::cast_slice(&curve));
            buf.extend_from_slice(&entry_count.to_le_bytes());
            for (tag, typ, cnt, off) in [
                (50708u16, 2u16, model.len() as u32, m_off),
                (50936, 2, name.len() as u32, n_off),
                (50937, 4, 3, hd_off),
                (50938, 11, hsm_data.len() as u32, h_off),
                (50940, 11, curve.len() as u32, c_off),
            ] {
                buf.extend_from_slice(&tag.to_le_bytes());
                buf.extend_from_slice(&typ.to_le_bytes());
                buf.extend_from_slice(&cnt.to_le_bytes());
                buf.extend_from_slice(&off.to_le_bytes());
            }
            buf.extend_from_slice(&0u32.to_le_bytes());
            buf.extend_from_slice(&pay);
            buf
        };
        let dcp_path = src_dir.join("sony_test.dcp");
        std::fs::write(&dcp_path, &dcp_bytes).unwrap();
        let dcp_desc = import_user_profile_in_dir(&store_dir, &dcp_path, None).unwrap();

        // 3) Synthetic .icc matrix-shaper (rXYZ/gXYZ/bXYZ + para gamma 1.18)
        let assemble_icc = |tags: &[([u8; 4], Vec<u8>)]| -> Vec<u8> {
            let mut hdr = vec![0u8; 128];
            hdr[12..16].copy_from_slice(b"scnr");
            hdr[16..20].copy_from_slice(b"RGB ");
            hdr[20..24].copy_from_slice(b"XYZ ");
            hdr[36..40].copy_from_slice(b"acsp");
            let mut tbl = (tags.len() as u32).to_be_bytes().to_vec();
            let start = 128 + 4 + tags.len() * 12;
            let mut pay = Vec::new();
            for (sig, data) in tags {
                tbl.extend_from_slice(sig);
                tbl.extend_from_slice(&((start + pay.len()) as u32).to_be_bytes());
                tbl.extend_from_slice(&(data.len() as u32).to_be_bytes());
                pay.extend_from_slice(data);
                while !pay.len().is_multiple_of(4) {
                    pay.push(0);
                }
            }
            let total = (128 + tbl.len() + pay.len()) as u32;
            hdr[0..4].copy_from_slice(&total.to_be_bytes());
            hdr.extend_from_slice(&tbl);
            hdr.extend_from_slice(&pay);
            hdr
        };
        let make_xyz = |v: [f32; 3]| -> Vec<u8> {
            let mut p = b"XYZ \0\0\0\0".to_vec();
            for c in v {
                p.extend_from_slice(&((c * 65536.0).round() as i32).to_be_bytes());
            }
            p
        };
        let mut para = b"para\0\0\0\0".to_vec();
        para.extend_from_slice(&0u16.to_be_bytes());
        para.extend_from_slice(&0u16.to_be_bytes());
        para.extend_from_slice(&((1.18_f32 * 65536.0).round() as i32).to_be_bytes());
        let icc_ms_bytes = assemble_icc(&[
            (*b"rXYZ", make_xyz([0.436_075, 0.222_505, 0.013_932])),
            (*b"gXYZ", make_xyz([0.385_065, 0.716_879, 0.097_105])),
            (*b"bXYZ", make_xyz([0.143_080, 0.060_617, 0.714_173])),
            (*b"rTRC", para.clone()),
            (*b"gTRC", para.clone()),
            (*b"bTRC", para),
        ]);
        let icc_ms_path = src_dir.join("matrix_shaper.icc");
        std::fs::write(&icc_ms_path, &icc_ms_bytes).unwrap();
        let icc_ms_desc =
            import_user_profile_in_dir(&store_dir, &icc_ms_path, Some("ICC Matrix Shaper"))
                .unwrap();

        // 4) Synthetic .icc A2B0 mft2 16-bit 3D CLUT
        let icc_mft2_bytes = {
            let grid = 7usize;
            let mut mft = b"mft2\0\0\0\0".to_vec();
            mft.extend_from_slice(&[3, 3, grid as u8, 0]);
            for v in [1.0_f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
                mft.extend_from_slice(&((v * 65536.0).round() as i32).to_be_bytes());
            }
            mft.extend_from_slice(&2u16.to_be_bytes());
            mft.extend_from_slice(&2u16.to_be_bytes());
            for _ in 0..3 {
                mft.extend_from_slice(&0u16.to_be_bytes());
                mft.extend_from_slice(&65535u16.to_be_bytes());
            }
            let srgb_d50 = [
                0.436_075_f32,
                0.385_065,
                0.143_080,
                0.222_505,
                0.716_879,
                0.060_617,
                0.013_932,
                0.097_105,
                0.714_173,
            ];
            for r in 0..grid {
                let fr = (r as f32 / (grid - 1) as f32).powf(0.90);
                for g in 0..grid {
                    let fg = g as f32 / (grid - 1) as f32;
                    for b in 0..grid {
                        let fb = (b as f32 / (grid - 1) as f32).powf(1.08);
                        let x = srgb_d50[0] * fr + srgb_d50[1] * fg + srgb_d50[2] * fb;
                        let y = srgb_d50[3] * fr + srgb_d50[4] * fg + srgb_d50[5] * fb;
                        let z = srgb_d50[6] * fr + srgb_d50[7] * fg + srgb_d50[8] * fb;
                        for c in [x, y, z] {
                            let u = (c * 32768.0).round().clamp(0.0, 65535.0) as u16;
                            mft.extend_from_slice(&u.to_be_bytes());
                        }
                    }
                }
            }
            for _ in 0..3 {
                mft.extend_from_slice(&0u16.to_be_bytes());
                mft.extend_from_slice(&65535u16.to_be_bytes());
            }
            assemble_icc(&[(*b"A2B0", mft)])
        };
        let icc_mft_path = src_dir.join("clut_mft2.icm");
        std::fs::write(&icc_mft_path, &icc_mft2_bytes).unwrap();
        let icc_mft_desc =
            import_user_profile_in_dir(&store_dir, &icc_mft_path, Some("ICC MFT2 CLUT")).unwrap();

        for profile_id in [
            cube_desc.id.as_str(),
            dcp_desc.id.as_str(),
            icc_ms_desc.id.as_str(),
            icc_mft_desc.id.as_str(),
        ] {
            for strength in [1.0_f32, 0.5_f32] {
                let prof_recipe = DevelopRecipe {
                    camera_profile: profile_id.to_owned(),
                    profile_strength: strength,
                    exposure: 0.1,
                    ..DevelopRecipe::default()
                };
                let gpu_prof = render_offscreen_rgba32f_with_profiles_dir(
                    &device,
                    &queue,
                    &mut gpu,
                    &ViewportCamera::default(),
                    &prof_recipe,
                    96,
                    96,
                    Some(&store_dir),
                );
                let (cw, ch, cpu_prof) =
                    rune_editor_decode::export::render_developed_srgb_f32_with_profiles_dir(
                        &tex_img,
                        &prof_recipe,
                        &rune_editor_decode::ExportOptions::default(),
                        Some(&store_dir),
                    )
                    .expect("render cpu user profile parity");
                assert_eq!((cw, ch), (40, 40));

                let mut max_diff = 0.0_f32;
                let mut worst = (0u32, 0u32, 0usize, 0.0_f32, 0.0_f32);
                for y in 2..38u32 {
                    for x in 2..38u32 {
                        let gp = gpu_prof[((28 + y) * 96 + (28 + x)) as usize];
                        let cp = cpu_prof[(y * cw + x) as usize];
                        for ch in 0..3 {
                            let gpu_srgb = rune_editor_core::linear_to_srgb(gp[ch]);
                            let diff = (gpu_srgb - cp[ch]).abs();
                            if diff > max_diff {
                                max_diff = diff;
                                worst = (x, y, ch, gpu_srgb, cp[ch]);
                            }
                        }
                    }
                }
                assert!(
                    max_diff < 2e-3,
                    "External user profile GPU vs CPU parity drift for '{profile_id}' (strength={strength}): max={max_diff:.6} at ({},{}) ch={} gpu={} cpu={}",
                    worst.0,
                    worst.1,
                    worst.2,
                    worst.3,
                    worst.4
                );
            }
        }

        // Verify GPU re-uploads when an existing user profile is overwritten on disk!
        let cube_recipe = DevelopRecipe {
            camera_profile: cube_desc.id.clone(),
            profile_strength: 1.0,
            ..DevelopRecipe::default()
        };
        let gpu_before = render_offscreen_rgba32f_with_profiles_dir(
            &device,
            &queue,
            &mut gpu,
            &ViewportCamera::default(),
            &cube_recipe,
            96,
            96,
            Some(&store_dir),
        );
        std::fs::write(&cube_path, build_cube("Cinema Cool", 1.25, 0.78)).unwrap();
        import_user_profile_in_dir_with_overwrite(
            &store_dir,
            &cube_path,
            Some("Cinema Warm"),
            true,
        )
        .unwrap();
        let gpu_after = render_offscreen_rgba32f_with_profiles_dir(
            &device,
            &queue,
            &mut gpu,
            &ViewportCamera::default(),
            &cube_recipe,
            96,
            96,
            Some(&store_dir),
        );
        let (_, _, cpu_after) =
            rune_editor_decode::export::render_developed_srgb_f32_with_profiles_dir(
                &tex_img,
                &cube_recipe,
                &rune_editor_decode::ExportOptions::default(),
                Some(&store_dir),
            )
            .unwrap();
        let center_before = gpu_before[48 * 96 + 48];
        let center_after = gpu_after[48 * 96 + 48];
        assert!(
            (center_before[0] - center_after[0]).abs() > 0.03
                && (center_before[2] - center_after[2]).abs() > 0.03,
            "GPU must re-upload overwritten user profile: before={center_before:?} after={center_after:?}"
        );
        for ch in 0..3 {
            let d = (rune_editor_core::linear_to_srgb(center_after[ch])
                - cpu_after[20 * 40 + 20][ch])
                .abs();
            assert!(
                d < 2e-3,
                "Overwritten user profile GPU vs CPU drift on ch {ch}: {d}"
            );
        }

        let _ = std::fs::remove_dir_all(&store_dir);
        let _ = std::fs::remove_dir_all(&src_dir);
    }
}
