// The stroke's appearance under one dab's read region: the pre-stroke
// snapshot with a terminal's packed grounds laid on it under the commit
// law, written into the scratch's appearance mirror. Dispatched once per
// dab, before that dab's own dispatch in the same compute pass, so a graph
// that samples the stroke at other pixels (`clone_source`'s Live arm) reads
// exactly what the commit would show after the previous dab.
//
// Full strength, paint mode: the stroke's opacity cap and its blend mode
// are the commit's, applied once, as for every `paint` brush. Applying
// them here too would scale the smear chain twice.
//
// Group 0 is the per-brush uniform group the brush's own pipeline has
// bound. This module declares nothing in it, so the group stays bound
// across the pipeline switch and only group 1 is rebound per dab.
//
// `WORKGROUP` is overridden with `DAB_WORKGROUP` at pipeline build, so the
// snapshot's grid and the dab's divide by one constant.
// Includes `source_over.wgsl`, `lib/deposit_ceiling.wgsl` and
// `lib/commit_law.wgsl`.

override WORKGROUP: u32 = 8u;

struct SnapshotUniforms {
    has_wash: u32,       // 0 = slot absent
    has_build: u32,      // 0 = slot absent
    law: u32,            // 0 = deposit, 1 = move (`lib/commit_law.wgsl`)
    _pad: u32,
};
// One per dab, in lockstep with the dab records: the read region, in
// write-side (layer-local) texels, already clamped to the layer.
struct SnapshotRecord { origin: vec2<u32>, size: vec2<u32> };
struct DabSlot { i: u32, pad0: u32, pad1: u32, pad2: u32 };

@group(1) @binding(0) var<uniform> flags: SnapshotUniforms;
@group(1) @binding(1) var<storage, read> records: array<SnapshotRecord>;
@group(1) @binding(2) var<uniform> slot: DabSlot;
@group(1) @binding(3) var wash: texture_2d<u32>;
@group(1) @binding(4) var build: texture_2d<u32>;
@group(1) @binding(5) var appearance: texture_storage_2d<rgba8unorm, write>;
@group(1) @binding(6) var pre_stroke: texture_2d<f32>;

@compute @workgroup_size(WORKGROUP, WORKGROUP, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let r = records[slot.i];
    if (any(gid.xy >= r.size)) {
        return;
    }
    let px = vec2<i32>(r.origin + gid.xy);
    let wash_px = unpack4x8unorm(textureLoad(wash, px, 0).r);
    let build_px = unpack4x8unorm(textureLoad(build, px, 0).r);
    let bg = textureLoad(pre_stroke, px, 0);
    textureStore(appearance, px, commit_law(wash_px, build_px, bg, 0u, flags.law,
                                            f32(flags.has_wash), f32(flags.has_build)));
}
