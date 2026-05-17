//! Headless GPU test utilities.
//!
//! Provides a `(Device, Queue)` pair without a window surface, plus helpers
//! for creating textures with known data and reading them back for assertions.

/// Create a headless wgpu device + queue for testing.
///
/// Uses wgpu's automatic backend selection — Vulkan, Metal, DX12, or software
/// fallback depending on the platform. No window surface required.
pub fn test_device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("no GPU adapter available for tests");

    block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("test-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))
    .expect("failed to create test device")
}

/// Minimal future executor — blocks the current thread until the future resolves.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    // wgpu futures resolve after a single poll on native backends.
    let waker = noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(val) => return val,
            std::task::Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn noop_waker() -> std::task::Waker {
    use std::task::{RawWaker, RawWakerVTable, Waker};
    const VTABLE: RawWakerVTable =
        RawWakerVTable::new(|p| RawWaker::new(p, &VTABLE), |_| {}, |_| {}, |_| {});
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

/// Create an RGBA8 texture with known pixel data.
pub fn create_test_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    data: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
    create_test_texture_with_format(
        device,
        queue,
        width,
        height,
        data,
        wgpu::TextureFormat::Rgba8Unorm,
    )
}

/// Create a texture with known pixel data and a specific format.
pub fn create_test_texture_with_format(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    data: &[u8],
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let bpp = format.block_copy_size(None).unwrap_or(1);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test-texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    if !data.is_empty() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * bpp),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Read back an entire texture to CPU memory (blocking). For test assertions.
pub fn readback_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("test-readback"),
    });
    let request = super::readback::request_readback(
        device,
        &mut encoder,
        texture,
        format,
        crate::coord::LayerRect::from_xywh(0, 0, width, height),
    );
    queue.submit([encoder.finish()]);
    request.blocking_read(device)
}
