/// Compute shader: find which pixels differ between two textures.
///
/// Compares `tex_a` (pre-operation snapshot) against `tex_b` (current layer).
/// Each thread checks one pixel. Every differing pixel folds into the atomic
/// min/max bounding rect, and every 16x16 workgroup containing one sets its
/// tile's bit in `flags` (tile dims are multiples of 16, so a workgroup never
/// straddles two tiles). After dispatch `out` holds [min_x, min_y, max_x,
/// max_y, flags...]; if min_x > max_x, the textures are identical.

@group(0) @binding(0) var tex_a: texture_2d<f32>;
@group(0) @binding(1) var tex_b: texture_2d<f32>;

struct DiffResult {
    min_x: atomic<u32>,
    min_y: atomic<u32>,
    max_x: atomic<u32>,
    max_y: atomic<u32>,
    /// One bit per tile, row major over the texture's tile grid.
    flags: array<atomic<u32>>,
}
@group(0) @binding(2) var<storage, read_write> out: DiffResult;

struct Params {
    width: u32,
    height: u32,
    tile_w: u32,
    tile_h: u32,
}
@group(0) @binding(3) var<uniform> params: Params;

var<workgroup> hit: atomic<u32>;

@compute @workgroup_size(16, 16)
fn main(
    @builtin(global_invocation_id) gid: vec3u,
    @builtin(local_invocation_index) li: u32,
) {
    // No early return: the barrier below must be reached in uniform control
    // flow by every invocation of the workgroup.
    var differs = false;
    if gid.x < params.width && gid.y < params.height {
        let coord = vec2u(gid.x, gid.y);
        differs = any(textureLoad(tex_a, coord, 0) != textureLoad(tex_b, coord, 0));
    }

    if differs {
        atomicMin(&out.min_x, gid.x);
        atomicMin(&out.min_y, gid.y);
        atomicMax(&out.max_x, gid.x);
        atomicMax(&out.max_y, gid.y);
        atomicOr(&hit, 1u);
    }

    workgroupBarrier();

    if li == 0u && atomicLoad(&hit) != 0u {
        let tiles_x = (params.width + params.tile_w - 1u) / params.tile_w;
        let tile = vec2u(gid.x / params.tile_w, gid.y / params.tile_h);
        let idx = tile.y * tiles_x + tile.x;
        atomicOr(&out.flags[idx / 32u], 1u << (idx % 32u));
    }
}
