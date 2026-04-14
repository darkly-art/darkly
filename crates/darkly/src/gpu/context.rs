pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: Option<wgpu::Surface<'static>>,
    pub surface_config: Option<wgpu::SurfaceConfiguration>,
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
            surface: Some(surface),
            surface_config: Some(surface_config),
            is_software,
        }
    }

    /// Create a headless GPU context — no surface or window needed.
    /// Used for testing and headless rendering.
    pub fn new_headless(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        GpuContext {
            device,
            queue,
            surface: None,
            surface_config: None,
            is_software: true,
        }
    }

    /// Create a command encoder, run `f`, and submit the resulting commands.
    ///
    /// Eliminates the 4-line boilerplate pattern that appears ~30 times in the
    /// engine: create encoder → do work → queue.submit.
    pub fn encode(&self, label: &str, f: impl FnOnce(&mut wgpu::CommandEncoder)) {
        let mut encoder = self.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some(label) },
        );
        f(&mut encoder);
        self.queue.submit([encoder.finish()]);
    }

    /// Like `encode`, but returns a value from the closure.
    pub fn encode_ret<T>(&self, label: &str, f: impl FnOnce(&mut wgpu::CommandEncoder) -> T) -> T {
        let mut encoder = self.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some(label) },
        );
        let result = f(&mut encoder);
        self.queue.submit([encoder.finish()]);
        result
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            if let (Some(surface), Some(config)) = (&self.surface, &mut self.surface_config) {
                config.width = width;
                config.height = height;
                surface.configure(&self.device, config);
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
