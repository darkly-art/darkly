//! The baked-source cache: equal field specs share one GPU tile, distinct
//! specs bake distinct tiles, and both channel layouts (grayscale `value`,
//! RGBA `color`) bake without error. That the baked tile carries real fBm
//! content is proven end-to-end by `noise_frame` (a dab-space grain that
//! rotates with the stamp can only come from a non-flat tile).

use std::sync::Arc;

use darkly::brush::texture_source::{BakeChannels, BakeKind, BakeSpec};
use darkly::gpu::baked_source_cache::BakedSourceCache;
use darkly::gpu::test_utils::{readback_texture, test_device};

fn noise_spec(seed: u32, channels: BakeChannels) -> BakeSpec {
    BakeSpec {
        kind: BakeKind::Noise {
            seed,
            octaves: 4,
            warp_q: BakeKind::quantize(0.6),
            roughness_q: BakeKind::quantize(0.5),
        },
        channels,
        resolution: BakeSpec::resolution_for_octaves(4),
    }
}

#[test]
fn bake_cache_dedups_equal_specs_and_separates_distinct() {
    let (device, queue) = test_device();
    let cache = BakedSourceCache::new();

    let spec_a = noise_spec(1, BakeChannels::Rgba);
    let a1 = cache.get_or_bake(&device, &queue, &spec_a);
    let a2 = cache.get_or_bake(&device, &queue, &spec_a);
    assert!(
        Arc::ptr_eq(&a1, &a2),
        "equal specs must share one cached tile",
    );
    assert_eq!(a1.width, spec_a.resolution);
    assert_eq!(a1.height, spec_a.resolution);

    // A different seed is a different field → a different tile.
    let b = cache.get_or_bake(&device, &queue, &noise_spec(2, BakeChannels::Rgba));
    assert!(
        !Arc::ptr_eq(&a1, &b),
        "distinct seeds must bake distinct tiles",
    );

    // A different channel layout (grayscale R8) is also a distinct tile and
    // bakes through the R8 pipeline without error.
    let gray = cache.get_or_bake(&device, &queue, &noise_spec(1, BakeChannels::Grayscale));
    assert!(!Arc::ptr_eq(&a1, &gray), "grayscale is a distinct tile");
    assert_eq!(
        gray.width,
        noise_spec(1, BakeChannels::Grayscale).resolution
    );
}

/// The baked tile is **seamless** under repeat wrap: the field is periodic, so
/// the last column is one continuous texel-step from the first. That wrap
/// difference must be the same order as an ordinary interior adjacent-column
/// step; a non-tileable (rotated) field would jump ~randomly across the seam.
#[test]
fn baked_tile_is_seamless_under_repeat_wrap() {
    let (device, queue) = test_device();
    let cache = BakedSourceCache::new();
    let spec = noise_spec(1, BakeChannels::Rgba);
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

    // Mean |Δ| across the wrap seam vs. across an interior adjacent step, on
    // both axes. Seamless ⇒ the two are the same order of magnitude.
    let mean = |f: &dyn Fn(usize) -> i32| (0..res).map(f).sum::<i32>() as f64 / res as f64;
    let wrap_x = mean(&|y| (r(0, y) - r(res - 1, y)).abs());
    let interior_x = mean(&|y| (r(0, y) - r(1, y)).abs());
    let wrap_y = mean(&|x| (r(x, 0) - r(x, res - 1)).abs());
    let interior_y = mean(&|x| (r(x, 0) - r(x, 1)).abs());

    assert!(
        wrap_x <= interior_x * 4.0 + 2.0,
        "vertical seam: mean wrap Δ {wrap_x:.2} vs interior Δ {interior_x:.2}",
    );
    assert!(
        wrap_y <= interior_y * 4.0 + 2.0,
        "horizontal seam: mean wrap Δ {wrap_y:.2} vs interior Δ {interior_y:.2}",
    );
}
