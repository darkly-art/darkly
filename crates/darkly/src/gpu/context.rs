use std::sync::Arc;

/// Shareable GPU device + queue. Multiple `GpuContext`s — and thus multiple
/// `DarklyEngine` instances rendering to different canvases — can hold an
/// `Arc<GpuDevice>` and use a single underlying WebGPU device. This avoids
/// duplicate adapter/device acquisition (mandatory on web, where browsers
/// typically expose only one device per origin) and lets shaders/pipelines
/// be compiled once per device rather than once per engine.
///
/// `wgpu::Device` and `wgpu::Queue` are `Send + Sync` on native but not on
/// wasm32. Darkly is single-threaded everywhere (the JS event loop on web,
/// the main thread on native), so the `Arc` is only ever used for shared
/// ownership across engines on the same thread — `Rc` would work too, but
/// we keep `Arc` so the `GpuDevice` type doesn't fork by platform. The
/// clippy `arc_with_non_send_sync` lint is suppressed at construction.
pub struct GpuDevice {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

pub struct GpuContext {
    pub gpu: Arc<GpuDevice>,
    pub surface: Option<wgpu::Surface<'static>>,
    pub surface_config: Option<wgpu::SurfaceConfiguration>,
}

// Field access through `Deref` lets the engine keep using `self.gpu.device`
// and `self.gpu.queue` everywhere — `self.gpu` is a `GpuContext`, has no
// `device` field, autoderefs to `GpuDevice`, finds it.
impl std::ops::Deref for GpuContext {
    type Target = GpuDevice;
    fn deref(&self) -> &GpuDevice {
        &self.gpu
    }
}

impl GpuContext {
    /// Create a GPU context from a pre-built instance and surface.
    ///
    /// The caller is responsible for platform-specific instance and surface
    /// creation (e.g. from an HTML canvas or a native window handle).
    /// `limits` controls device capability requirements (e.g.
    /// `Limits::downlevel_webgl2_defaults()` for WASM,
    /// `Limits::default()` for native).
    pub async fn new(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        limits: wgpu::Limits,
        initial_width: u32,
        initial_height: u32,
    ) -> Self {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find a suitable GPU adapter");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("darkly-device"),
                required_features: wgpu::Features::empty(),
                required_limits: limits.using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .expect("Failed to create device");

        let surface_config =
            configure_surface(&surface, &adapter, &device, initial_width, initial_height);

        GpuContext {
            #[allow(clippy::arc_with_non_send_sync)] // see GpuDevice docs
            gpu: Arc::new(GpuDevice { device, queue }),
            surface: Some(surface),
            surface_config: Some(surface_config),
        }
    }

    /// Build a context that re-uses an existing shared `GpuDevice`. Use this
    /// to attach a second (or Nth) canvas to the same device — e.g. for the
    /// multi-tab editor. Picks the surface format the same way as `new`, but
    /// does not allocate a new device or queue.
    pub async fn new_with_shared_device(
        gpu: Arc<GpuDevice>,
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        initial_width: u32,
        initial_height: u32,
    ) -> Self {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find a suitable GPU adapter");

        let surface_config = configure_surface(
            &surface,
            &adapter,
            &gpu.device,
            initial_width,
            initial_height,
        );

        GpuContext {
            gpu,
            surface: Some(surface),
            surface_config: Some(surface_config),
        }
    }

    /// Create a headless GPU context — no surface or window needed.
    /// Used for testing and headless rendering.
    pub fn new_headless(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        GpuContext {
            #[allow(clippy::arc_with_non_send_sync)] // see GpuDevice docs
            gpu: Arc::new(GpuDevice { device, queue }),
            surface: None,
            surface_config: None,
        }
    }

    /// Like `new_headless`, but reuses an existing shared `GpuDevice`. The
    /// shared-device multi-engine integration test uses this to construct two
    /// engines on the same device without a surface.
    pub fn new_headless_shared(gpu: Arc<GpuDevice>) -> Self {
        GpuContext {
            gpu,
            surface: None,
            surface_config: None,
        }
    }

    /// Cheap clone of the underlying shared device handle. Use this when
    /// constructing a sibling engine that should render to the same WebGPU
    /// device as this one.
    pub fn shared_device(&self) -> Arc<GpuDevice> {
        Arc::clone(&self.gpu)
    }

    /// Create a command encoder, run `f`, and submit the resulting commands.
    ///
    /// Eliminates the 4-line boilerplate pattern that appears ~30 times in the
    /// engine: create encoder → do work → queue.submit.
    pub fn encode(&self, label: &str, f: impl FnOnce(&mut wgpu::CommandEncoder)) {
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        f(&mut encoder);
        self.gpu.queue.submit([encoder.finish()]);
    }

    /// Like `encode`, but returns a value from the closure.
    pub fn encode_ret<T>(&self, label: &str, f: impl FnOnce(&mut wgpu::CommandEncoder) -> T) -> T {
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        let result = f(&mut encoder);
        self.gpu.queue.submit([encoder.finish()]);
        result
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            if let (Some(surface), Some(config)) = (&self.surface, &mut self.surface_config) {
                config.width = width;
                config.height = height;
                surface.configure(&self.gpu.device, config);
            }
        }
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        match &self.surface_config {
            Some(config) => config.format,
            // Headless fallback — Bgra8UnormSrgb is the most common desktop
            // surface format, so pipelines compiled against it will match
            // production behaviour.
            None => wgpu::TextureFormat::Bgra8UnormSrgb,
        }
    }

    /// True when running headless (no presentation surface).
    pub fn is_headless(&self) -> bool {
        self.surface.is_none()
    }
}

fn configure_surface(
    surface: &wgpu::Surface<'static>,
    adapter: &wgpu::Adapter,
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> wgpu::SurfaceConfiguration {
    let surface_caps = surface.get_capabilities(adapter);
    let surface_format = surface_caps
        .formats
        .iter()
        .find(|f| f.is_srgb())
        .copied()
        .unwrap_or(surface_caps.formats[0]);

    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        format: surface_format,
        width,
        height,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: surface_caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(device, &config);
    config
}
