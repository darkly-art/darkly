/// Compute shader: find the bounding rect of pixels that differ between two textures.
///
/// Compares `tex_a` (pre-stroke scratch) against `tex_b` (post-stroke canvas).
/// Each thread checks one pixel and updates shared atomic min/max for any
/// differing pixel. After dispatch, the storage buffer holds [min_x, min_y,
/// max_x, max_y]. If min_x > max_x, the textures are identical (no diff).

@group(0) @binding(0) var tex_a: texture_2d<f32>;
@group(0) @binding(1) var tex_b: texture_2d<f32>;

struct Bounds {
    min_x: atomic<u32>,
    min_y: atomic<u32>,
    max_x: atomic<u32>,
    max_y: atomic<u32>,
}
@group(0) @binding(2) var<storage, read_write> bounds: Bounds;

struct Params {
    width: u32,
    height: u32,
}
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    if gid.x >= params.width || gid.y >= params.height { return; }

    let coord = vec2u(gid.x, gid.y);
    let a = textureLoad(tex_a, coord, 0);
    let b = textureLoad(tex_b, coord, 0);

    if any(a != b) {
        atomicMin(&bounds.min_x, gid.x);
        atomicMin(&bounds.min_y, gid.y);
        atomicMax(&bounds.max_x, gid.x);
        atomicMax(&bounds.max_y, gid.y);
    }
}
