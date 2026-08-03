//! Regression: the baked `noise` field must not visibly repeat at the tiny
//! period it used to. The field is baked once and Repeat-sampled to cover the
//! plane, so its repeat period is `BakeSpec::FIELD_SPAN` field units. When that
//! period was 16 field units the grain visibly tiled; the fix makes the period
//! large (`FIELD_SPAN = 128`) while leaving the tile *resolution* (memory)
//! unchanged — the texels stretch across a longer period rather than the tile
//! growing.
//!
//! This test fails against the old 16-unit field and passes against the fixed
//! one, without asserting the constant itself: it bakes the real tile and checks
//! that a shift of *16 field units* — the entire old repeat period — no longer
//! reproduces the field. Under the old period that shift was a full wrap
//! (perfect self-correlation); under the large period it lands on unrelated
//! content, as decorrelated as a reference taken half the tile away.

use darkly::brush::texture_source::{BakeChannels, BakeKind, BakeSpec};
use darkly::gpu::baked_source_cache::BakedSourceCache;
use darkly::gpu::test_utils::{readback_texture, test_device};

/// The `noise` node's default field (octaves=4, chromatic), matching the
/// baked-source cache's other tests.
fn default_noise_spec() -> BakeSpec {
    BakeSpec {
        kind: BakeKind::Noise {
            seed: 1,
            octaves: 4,
            warp_q: BakeKind::quantize(0.6),
            roughness_q: BakeKind::quantize(0.5),
        },
        channels: BakeChannels::Rgba,
        resolution: BakeSpec::resolution_for_octaves(4),
    }
}

#[test]
fn baked_field_does_not_repeat_at_the_old_period() {
    let (device, queue) = test_device();
    let cache = BakedSourceCache::new();
    let spec = default_noise_spec();
    let tile = cache.get_or_bake(&device, &queue, &spec);
    let res = spec.resolution as usize;
    let px = readback_texture(
        &device,
        &queue,
        &tile.texture,
        wgpu::TextureFormat::Rgba8Unorm,
        res as u32,
        res as u32,
    );
    let r = |x: usize, y: usize| px[(y * res + x) * 4] as i32;

    // How many texels correspond to the old 16-field-unit repeat period. The
    // whole tile spans FIELD_SPAN field units across `res` texels, so 16 field
    // units is `res * 16 / FIELD_SPAN` texels. At FIELD_SPAN=16 this is the
    // full tile width (a wrap-around → identical); at 128 it is res/8.
    let old_period_units = 16.0_f32;
    // At FIELD_SPAN=16 this equals `res` (a full wrap → `% res` compares each
    // column with itself → Δ 0), which is exactly the repeat this test rejects.
    let shift = (res as f32 * old_period_units / BakeSpec::FIELD_SPAN).round() as usize;
    assert!(shift > 0, "old-period shift must be positive");

    // Mean |Δ| between each column and the one `shift` texels away (the old
    // repeat offset), and — as a self-calibrating "unrelated" reference on the
    // same backend — between each column and the one half a tile away. Half a
    // tile is 8 field units under the old period and 64 under the new, so it is
    // guaranteed-unrelated content either way.
    let far = res / 2;
    let mean = |off: usize| {
        let mut acc = 0i64;
        for y in 0..res {
            for x in 0..res {
                acc += (r(x, y) - r((x + off) % res, y)).abs() as i64;
            }
        }
        acc as f64 / (res * res) as f64
    };
    let delta_old_period = mean(shift);
    let delta_unrelated = mean(far);

    // The field must change substantially over the old period — as much as it
    // does over a known-unrelated distance. Under the old 16-unit period the
    // shift was a full wrap, so `delta_old_period` was ~0 and this fails.
    assert!(
        delta_old_period > delta_unrelated * 0.5,
        "field still self-similar at the old 16-unit period: Δ@old-period {delta_old_period:.2} \
         vs Δ@unrelated {delta_unrelated:.2} — the grain visibly repeats",
    );
}
