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

        // Upload linear Rgba16Float photo texture once per image generation, 256x1 curve LUT
        // when tone curves change, 33x33x33 Rgba16Float camera profile 3D LUT when profile
        // changes (O(1) Arc::ptr_eq cache), and 1024x512 R8Unorm layer mask atlas when layers change.
        let generation = f.rt.state.image_generation;
        gpu.ensure_image_uploaded(f.device, f.queue, generation, &f.rt.state.image);
        gpu.ensure_curve_lut_uploaded(f.queue, &f.rt.state.recipe);
        gpu.ensure_profile_lut_uploaded(f.queue, &f.rt.state.recipe);
        gpu.ensure_layer_masks_uploaded(f.queue, &f.rt.state.recipe);

        // Pre-build Copy uniform block by borrowing recipe (zero per-frame heap allocations).
        let base_uniforms = DevelopUniforms::build(
            [0.0; 4],
            [f.size.0, f.size.1],
            gpu.image_size(),
            &f.rt.state.camera,
            &f.rt.state.recipe,
            f.srgb_blend(),
        );

        let device = f.device;
        let queue = f.queue;
        let clear = f.rt.clear_color();
        f.ui.set_fade_stops(f.rt.fade_stops());
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);

        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
            if site.mark != PHOTO_VIEWPORT_MARK {
                return;
            }
            let mut site_uniforms = base_uniforms;
            site_uniforms.slot_rect = [
                site.instance.rect[0] - site.origin[0],
                site.instance.rect[1] - site.origin[1],
                site.instance.rect[2],
                site.instance.rect[3],
            ];
            site_uniforms.dims[0] = site.size[0] as f32;
            site_uniforms.dims[1] = site.size[1] as f32;
            site_uniforms.flags[3] = if site.srgb_blend { 1.0 } else { 0.0 };
            queue.write_buffer(gpu.uniform_buffer(), 0, bytemuck::bytes_of(&site_uniforms));
            gpu.draw(device, site.format, pass);
        });
    }

    fn release_gpu(&mut self) {
        self.gpu = None;
        self.device_key = None;
    }
}
