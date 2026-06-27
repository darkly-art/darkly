/// Compute shader: scan a texture for the bounding box of non-transparent content.
///
/// Dispatched over the full texture dimensions. Each thread checks one pixel
/// and updates shared atomic min/max values. After dispatch, the storage
/// buffer holds [min_x, min_y, max_x, max_y]. If min_x > max_x the texture
/// is fully transparent (no content).

@group(0) @binding(0) var tex: texture_2d<f32>;

struct Bounds {
    min_x: atomic<u32>,
    min_y: atomic<u32>,
    max_x: atomic<u32>,
    max_y: atomic<u32>,
}
@group(0) @binding(1) var<storage, read_write> bounds: Bounds;

struct Params {
    width: u32,
    height: u32,
    /// 0 = check alpha channel (RGBA layers), 1 = check red channel (R8 masks).
    use_r_channel: u32,
}
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    if gid.x >= params.width || gid.y >= params.height { return; }

    let pixel = textureLoad(tex, vec2u(gid.x, gid.y), 0);
    let has_content = select(pixel.a > 0.0, pixel.r > 0.0, params.use_r_channel != 0u);

    if has_content {
        atomicMin(&bounds.min_x, gid.x);
        atomicMin(&bounds.min_y, gid.y);
        atomicMax(&bounds.max_x, gid.x);
        atomicMax(&bounds.max_y, gid.y);
    }
}
