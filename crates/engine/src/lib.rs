//! Portable `wgpu` linear GPU photo develop engine.
//!
//! Designed to execute inside Rune's `Compositor<S>` (`EffectSite` slot) directly
//! on macOS `objc2-metal` (via Rune's custom Metal backend) as well as DX12/Vulkan.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use rune_editor_core::{DevelopRecipe, ViewportCamera};
use rune_editor_decode::LinearImage;

const DEVELOP_WGSL: &str = include_str!("../../../shaders/develop.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DevelopUniforms {
    pub slot_rect: [f32; 4],
    pub dims: [f32; 4],
    pub camera: [f32; 4],
    pub tone_a: [f32; 4],
    pub tone_b: [f32; 4],
    pub color_fx: [f32; 4],
    pub flags: [f32; 4],
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

        Self {
            shader,
            bind_group_layout,
            pipeline_layout,
            pipelines: HashMap::new(),
            uniform_buffer,
            sampler,
            photo_texture: None,
            bind_group: None,
            uploaded_generation: 0,
            uploaded_size: [1, 1],
        }
    }

    pub fn uniform_buffer(&self) -> &wgpu::Buffer {
        &self.uniform_buffer
    }

    /// Uploads the scene-referred `Rgba16Float` image texture only when `generation` changes.
    pub fn ensure_image_uploaded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        generation: u64,
        image: &LinearImage,
    ) {
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
                        blend: Some(wgpu::BlendState::REPLACE),
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
