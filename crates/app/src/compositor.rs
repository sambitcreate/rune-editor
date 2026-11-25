//! Rune `Compositor<AppState>` hosting our single-pass linear `Rgba16Float` WGSL develop shader.

use rune::compose::{Compositor, DeviceKey, Frame};
use rune_editor_engine::{DevelopGpuResources, DevelopUniforms};

use crate::state::{AppState, PHOTO_VIEWPORT_MARK};

#[derive(Default)]
pub struct DarkroomCompositor {
    device_key: Option<DeviceKey>,
    gpu: Option<DevelopGpuResources>,
}

impl Compositor<AppState> for DarkroomCompositor {
    fn render(&mut self, f: &mut Frame<'_, AppState>) {
        let key = f.device_key();
        if self.device_key != Some(key) {
            self.gpu = Some(DevelopGpuResources::new(f.device));
            self.device_key = Some(key);
        }

        let Some(gpu) = self.gpu.as_mut() else {
            f.draw_ui();
            return;
        };

        // Upload linear Rgba16Float photo texture once per image generation.
        let generation = f.rt.state.image_generation;
        let image = f.rt.state.image.clone();
        gpu.ensure_image_uploaded(f.device, f.queue, generation, &image);

        // Pre-compute the viewport slot rect and write uniforms + 256x1 tone curve LUT
        // ahead of the render pass so zero extra command buffers are created mid-pass.
        let recipe = f.rt.state.recipe.clone();
        gpu.ensure_curve_lut_uploaded(f.queue, &recipe);
        let camera = f.rt.state.camera;
        let srgb_blend = f.srgb_blend();
        let image_size = gpu.image_size();
        let target_size = [f.size.0, f.size.1];

        let slot_rect = f
            .rt
            .instances
            .iter()
            .find(|inst| inst.params[1] as u32 == 6 && inst.uv[0].to_bits() == PHOTO_VIEWPORT_MARK)
            .map(|inst| inst.rect)
            .unwrap_or([0.0, 0.0, f.size.0 as f32, f.size.1 as f32]);

        let uniforms = DevelopUniforms::build(
            slot_rect,
            target_size,
            image_size,
            &camera,
            &recipe,
            srgb_blend,
        );
        f.write_buffer(gpu.uniform_buffer(), 0, bytemuck::bytes_of(&uniforms));

        let device = f.device;
        let queue = f.queue;
        let clear = f.rt.clear_color();
        f.ui.set_fade_stops(f.rt.fade_stops());
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);

        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
            if site.mark != PHOTO_VIEWPORT_MARK {
                return;
            }
            // Update uniforms with exact pass-local slot rect if inside an isolated layer or shifted origin.
            let local_rect = [
                site.instance.rect[0] - site.origin[0],
                site.instance.rect[1] - site.origin[1],
                site.instance.rect[2],
                site.instance.rect[3],
            ];
            let site_uniforms = DevelopUniforms::build(
                local_rect,
                site.size,
                image_size,
                &camera,
                &recipe,
                site.srgb_blend,
            );
            queue.write_buffer(gpu.uniform_buffer(), 0, bytemuck::bytes_of(&site_uniforms));
            gpu.draw(device, site.format, pass);
        });
    }

    fn release_gpu(&mut self) {
        self.gpu = None;
        self.device_key = None;
    }
}
