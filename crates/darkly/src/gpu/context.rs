pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
    /// True when running on a software renderer (e.g. llvmpipe, SwiftShader).
    /// Determined by the caller (platform layer) and passed in at construction.
    pub is_software: bool,
}

impl GpuContext {
    /// Create a GPU context from a pre-built instance and surface.
    ///
    /// The caller is responsible for platform-specific instance and surface
    /// creation (e.g. from an HTML canvas or a native window handle).
    /// `limits` controls device capability requirements (e.g.
    /// `Limits::downlevel_webgl2_defaults()` for WASM,
    /// `Limits::default()` for native).
    ///
    /// `is_software` should be set by the platform layer — e.g. on the web,
    /// via `adapter.info.isFallbackAdapter` or renderer string matching.
    pub async fn new(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        limits: wgpu::Limits,
        initial_width: u32,
        initial_height: u32,
        is_software: bool,
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

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format: surface_format,
            width: initial_width,
            height: initial_height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        log::info!("GPU context: is_software = {is_software}");

        GpuContext {
            device,
            queue,
            surface,
            surface_config,
            is_software,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
        }
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }
}
