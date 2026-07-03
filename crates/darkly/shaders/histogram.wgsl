// Compute shader: bin a texture into eight 256-bin histograms — one per virtual
// channel, matching the LUT filter's channel order:
//
//   0 rgb (composite), 1 red, 2 green, 3 blue, 4 alpha,
//   5 hue, 6 saturation, 7 lightness
//
// Each thread bins one pixel with atomic adds into an 8×256 u32 storage buffer.
//
// Prior art (Krita `KisLevelsConfigWidget` / `KoBasicHistogramProducers`):
//   - R/G/B/A bin the raw gamma-encoded 8-bit value directly (no linearization)
//     — `KoGenericRGBHistogramProducer::addRegionToBin` bins `c.red()` etc.
//   - The composite/default and Lightness channels both bin CIELAB L*
//     (`KoGenericLabHistogramProducer`, D65, channel 0, L*/100 → 0..255).
// Hue/Saturation use the same HSV conversion the filter applies. `rgb_to_hsv`
// and `rgb_to_lab` are prepended from `shaders/lib/colorspace.wgsl` at load time.

@group(0) @binding(0) var tex: texture_2d<f32>;

// 8 channels × 256 bins, laid out channel-major: bins[channel * 256 + bin].
struct Hist {
    bins: array<atomic<u32>, 2048>,
}
@group(0) @binding(1) var<storage, read_write> hist: Hist;

struct Params {
    width: u32,
    height: u32,
}
@group(0) @binding(2) var<uniform> params: Params;

fn bin_of(v: f32) -> u32 {
    return u32(clamp(round(v * 255.0), 0.0, 255.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3u) {
    if gid.x >= params.width || gid.y >= params.height { return; }

    let c = textureLoad(tex, vec2u(gid.x, gid.y), 0);

    // Composite (0) and Lightness (7) both bin CIELAB L*, as Krita does.
    let lightness = bin_of(rgb_to_lab(c.rgb).x / 100.0);
    atomicAdd(&hist.bins[0u * 256u + lightness], 1u);
    atomicAdd(&hist.bins[7u * 256u + lightness], 1u);

    // Raw gamma-encoded channel values.
    atomicAdd(&hist.bins[1u * 256u + bin_of(c.r)], 1u);
    atomicAdd(&hist.bins[2u * 256u + bin_of(c.g)], 1u);
    atomicAdd(&hist.bins[3u * 256u + bin_of(c.b)], 1u);
    atomicAdd(&hist.bins[4u * 256u + bin_of(c.a)], 1u);

    let hsv = rgb_to_hsv(c.rgb);
    atomicAdd(&hist.bins[5u * 256u + bin_of(hsv.x / 360.0)], 1u);
    atomicAdd(&hist.bins[6u * 256u + bin_of(hsv.y)], 1u);
}
