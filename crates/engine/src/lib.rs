//! Portable `wgpu` linear GPU photo develop engine.
//!
//! Designed to execute inside Rune's `Compositor<S>` (`EffectSite` slot) directly
//! on macOS `objc2-metal` (via Rune's custom Metal backend) as well as DX12/Vulkan.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use rune_editor_core::{CURVE_LUT_SIZE, DevelopRecipe, HSL_BAND_COUNT, ToneCurves, ViewportCamera};
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
    /// Spatial detail controls `[clarity, sharpen, noise_reduction, 0.0]`.
    pub detail: [f32; 4],
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
        let mut hsl = [[0.0_f32; 4]; HSL_BAND_COUNT];
        for (dst, band) in hsl.iter_mut().zip(recipe.hsl.iter()) {
            *dst = band.to_uniform();
        }

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
            shadows_wheel: recipe.shadows_wheel.to_uniform(),
            midtones_wheel: recipe.midtones_wheel.to_uniform(),
            highlights_wheel: recipe.highlights_wheel.to_uniform(),
            hsl,
            detail: [recipe.clarity, recipe.sharpen, recipe.noise_reduction, 0.0],
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

    /// Uploads the scene-referred `Rgba16Float` image texture only when `generation` changes.
    /// Also initializes the tone curve LUT to an identity ramp on first upload if
    /// `ensure_curve_lut_uploaded` has not yet been called.
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
    use rune_editor_core::{ColorWheel, HslBand, ToneCurves, evaluate_pixel_linear};

    #[test]
    fn develop_uniforms_alignment_and_packing() {
        let size = std::mem::size_of::<DevelopUniforms>();
        assert_eq!(size, 304);
        assert_eq!(size % 16, 0);

        let mut recipe = DevelopRecipe {
            shadows_wheel: ColorWheel::new(-0.2, -0.3, -0.1),
            midtones_wheel: ColorWheel::new(0.1, 0.05, 0.02),
            highlights_wheel: ColorWheel::new(0.3, 0.2, 0.04),
            clarity: 0.35,
            sharpen: 0.5,
            noise_reduction: 0.25,
            ..DevelopRecipe::default()
        };
        recipe.hsl[5] = HslBand::new(0.15, -0.4, -0.2);

        let u = DevelopUniforms::build(
            [0.0, 0.0, 1280.0, 800.0],
            [1280, 800],
            [1600, 1066],
            &ViewportCamera::default(),
            &recipe,
            true,
        );
        assert_eq!(u.shadows_wheel, [-0.2, -0.3, -0.1, 0.0]);
        assert_eq!(u.midtones_wheel, [0.1, 0.05, 0.02, 0.0]);
        assert_eq!(u.highlights_wheel, [0.3, 0.2, 0.04, 0.0]);
        assert_eq!(u.hsl[5], [0.15, -0.4, -0.2, 0.0]);
        assert_eq!(u.detail, [0.35, 0.5, 0.25, 0.0]);
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

    #[test]
    fn gpu_shader_compiles_and_matches_cpu_color_science() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = block_on_simple(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            ..Default::default()
        })) else {
            // Skip headless GPU render check if no GPU adapter is present.
            return;
        };
        let Ok((device, queue)) =
            block_on_simple(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            return;
        };

        let mut gpu = DevelopGpuResources::new(&device);

        // Build a uniform solid-color 16x16 linear test image so center pixel has zero edge/filter bleed.
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
        };

        // Non-trivial recipe exercising WB, Exposure, Tones, Contrast, Tone Curves,
        // 3-Way Color Balance, 8-Band HSL, and Vibrance/Saturation simultaneously.
        let mut recipe = DevelopRecipe {
            exposure: 0.35,
            contrast: 0.22,
            highlights: -0.25,
            shadows: 0.20,
            whites: 0.08,
            blacks: -0.06,
            temperature: 0.18,
            tint: -0.08,
            vibrance: 0.25,
            saturation: 0.10,
            vignette: 0.0,
            grain: 0.0,
            shadows_wheel: ColorWheel::new(-0.18, -0.24, -0.05),
            midtones_wheel: ColorWheel::new(0.12, 0.06, 0.04),
            highlights_wheel: ColorWheel::new(0.25, 0.15, 0.02),
            curves: ToneCurves {
                luma: vec![[0.0, 0.02], [0.25, 0.20], [0.75, 0.82], [1.0, 0.98]],
                red: vec![[0.0, 0.0], [0.5, 0.54], [1.0, 1.0]],
                blue: vec![[0.0, 0.02], [0.5, 0.47], [1.0, 0.96]],
                ..ToneCurves::default()
            },
            clarity: 0.0,
            sharpen: 0.0,
            noise_reduction: 0.0,
            ..DevelopRecipe::default()
        };
        recipe.hsl[5] = HslBand::new(-0.12, 0.18, -0.10);
        recipe.hsl[6] = HslBand::new(0.08, -0.15, 0.08);

        gpu.ensure_image_uploaded(&device, &queue, 1, &img);
        gpu.ensure_curve_lut_uploaded(&queue, &recipe);
        // Second call with identical curves should be a no-op.
        gpu.ensure_curve_lut_uploaded(&queue, &recipe);

        // Render to a 64x64 Rgba32Float offscreen target (linear output: srgb_blend = false).
        let target_w = 64u32;
        let target_h = 64u32;
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

        let camera = ViewportCamera {
            zoom: 4.0, // Fill entire 64x64 target with the center of the photo
            ..ViewportCamera::default()
        };
        let uniforms = DevelopUniforms::build(
            [0.0, 0.0, target_w as f32, target_h as f32],
            [target_w, target_h],
            gpu.image_size(),
            &camera,
            &recipe,
            false,
        );
        queue.write_buffer(gpu.uniform_buffer(), 0, bytemuck::bytes_of(&uniforms));

        // 64 pixels * 16 bytes/px = 1024 bytes per row (multiple of 256).
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
            gpu.draw(&device, wgpu::TextureFormat::Rgba32Float, &mut pass);
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
        let pixels: &[[f32; 4]] = bytemuck::cast_slice(&data);
        let center_gpu =
            pixels[(target_h as usize / 2) * target_w as usize + (target_w as usize / 2)];
        let cpu = evaluate_pixel_linear(src_rgb, &recipe);

        for ch in 0..3 {
            assert!(
                (center_gpu[ch] - cpu[ch]).abs() < 2e-3,
                "GPU/CPU parity drift on channel {ch}: gpu={} vs cpu={} (gpu={center_gpu:?}, cpu={cpu:?})",
                center_gpu[ch],
                cpu[ch]
            );
        }
        drop(data);
        readback.unmap();
    }
}
