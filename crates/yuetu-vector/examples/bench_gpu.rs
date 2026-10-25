//! Headless Vello GPU benchmark using a synthetic 20k-rectangle scene.
//!
//! This intentionally stays outside the app: it measures whether the wgpu
//! backend is usable on this machine before committing to a GPU scene path.

use std::time::Instant;

use vello_common::kurbo::Rect;
use vello_common::peniko::Color;
use vello_gpu::{ClearSettings, Scene, TargetInit};

const FRAMES: u32 = 20;

fn main() {
    pollster::block_on(run());
}

async fn run() {
    let mut args = std::env::args().skip(1);
    let width = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1600);
    let height = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1200);
    let mut scene = Scene::new(width, height);
    for index in 0..20_000u32 {
        let x = (index * 137) % 1550;
        let y = (index * 73) % 1150;
        let color = Color::from_rgb8(
            (index % 255) as u8,
            ((index * 7) % 255) as u8,
            ((index * 13) % 255) as u8,
        );
        scene.set_paint(color);
        scene.fill_rect(&Rect::new(
            f64::from(x),
            f64::from(y),
            f64::from(x) + 24.0,
            f64::from(y) + 18.0,
        ));
    }

    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        })
        .await
        .expect("no GPU adapter");
    println!("adapter: {:?}", adapter.get_info());
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("vello benchmark"),
            required_features: wgpu::Features::empty(),
            ..Default::default()
        })
        .await
        .expect("failed to create device");

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vello benchmark target"),
        size: wgpu::Extent3d {
            width: width.into(),
            height: height.into(),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let (mut renderer, mut resources) = vello_gpu::Renderer::new(
        &device,
        &vello_gpu::RenderTargetConfig {
            format,
            width,
            height,
        },
    );
    let size = vello_gpu::RenderSize { width, height };
    let depth = vello_gpu::Renderer::create_depth_texture_view(&device, &size);

    let mut times = Vec::with_capacity(FRAMES as usize);
    for frame in 0..FRAMES {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vello benchmark encoder"),
        });
        let start = Instant::now();
        renderer
            .render(
                &scene,
                &mut resources,
                &device,
                &queue,
                &mut encoder,
                &size,
                &view,
                Some(&depth),
                &vello_gpu::TextureBindings::new(),
                TargetInit::Clear(ClearSettings::default()),
            )
            .expect("vello GPU render failed");
        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll failed");
        if frame > 0 {
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let best = times.iter().copied().fold(f64::INFINITY, f64::min);

    let bytes_per_row = (u32::from(width) * 4).next_multiple_of(256);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vello benchmark readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("vello benchmark readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: width.into(),
            height: height.into(),
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("readback poll failed");
    let painted = readback
        .slice(..)
        .get_mapped_range()
        .unwrap()
        .chunks_exact(bytes_per_row as usize)
        .flat_map(|row| row[..usize::from(width) * 4].chunks_exact(4))
        .filter(|pixel| pixel[3] != 0)
        .count();
    readback.unmap();
    println!(
        "vello_gpu {width}x{height} · 20000 shapes: best {best:.2} ms, mean {mean:.2} ms · {painted} painted pixels"
    );
}
