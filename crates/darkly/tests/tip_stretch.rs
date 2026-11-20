//! The tip stretch on synthetic textures: the strongest texel's deviation
//! from the background reaches full contrast, every other texel scales by
//! the same factor, background stays background, alpha is untouched, and a
//! float source is read as an 8-bit target would have stored it.

use darkly::gpu::test_utils::{create_test_texture_with_format, readback_texture, test_device};
use darkly::gpu::tip_stretch::{TipStretch, TipStretchRefs};
use half::f16;

/// Stretch a one-row source of `texels` (in `format`) and read the result.
fn stretch(format: wgpu::TextureFormat, bytes: &[u8], width: u32, refs: TipStretchRefs) -> Vec<u8> {
    let (device, queue) = test_device();
    let (_src, src_view) =
        create_test_texture_with_format(&device, &queue, width, 1, bytes, format);
    let dst = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("tip-stretch-test-dst"),
        size: wgpu::Extent3d {
            width,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let dst_view = dst.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    TipStretch::new(&device).encode(
        &device,
        &mut encoder,
        &src_view,
        &dst_view,
        (width, 1),
        refs,
    );
    queue.submit([encoder.finish()]);
    readback_texture(
        &device,
        &queue,
        &dst,
        wgpu::TextureFormat::Rgba8Unorm,
        width,
        1,
    )
}

fn unorm(texels: &[[u8; 4]]) -> (Vec<u8>, u32) {
    (texels.concat(), texels.len() as u32)
}

/// Grey float texels with alpha 1, as f16 bytes.
fn float(values: &[f32]) -> (Vec<u8>, u32) {
    let bytes = values
        .iter()
        .flat_map(|&v| [v, v, v, 1.0])
        .flat_map(|c| f16::from_f32(c).to_le_bytes())
        .collect();
    (bytes, values.len() as u32)
}

fn reds(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>().0.iter().map(|px| px[0]).collect()
}

#[test]
fn empty_source_is_identity() {
    let (bytes, w) = unorm(&[[0, 0, 0, 0]; 4]);
    let out = stretch(
        wgpu::TextureFormat::Rgba8Unorm,
        &bytes,
        w,
        TipStretchRefs::COVERAGE,
    );
    assert_eq!(out, bytes);
}

#[test]
fn source_already_at_full_contrast_is_identity() {
    let (bytes, w) = unorm(&[[255, 255, 255, 255], [128, 128, 128, 200], [0, 0, 0, 0]]);
    let out = stretch(
        wgpu::TextureFormat::Rgba8Unorm,
        &bytes,
        w,
        TipStretchRefs::COVERAGE,
    );
    assert_eq!(out, bytes);
}

#[test]
fn deviation_from_the_background_stretches_to_full_contrast() {
    let bg = [20, 30, 40];
    let refs = TipStretchRefs::over([255, 255, 255], bg);
    // Peak deviation 50 (blue channel of the second texel); full is 235.
    let (bytes, w) = unorm(&[[20, 30, 40, 255], [40, 60, 90, 128], [30, 35, 65, 7]]);
    let out = stretch(wgpu::TextureFormat::Rgba8Unorm, &bytes, w, refs);
    let k = 235.0 / 50.0;
    let expect = |px: [u8; 4]| -> [u8; 4] {
        let c = |i: usize| {
            (bg[i] as f32 + (px[i] as f32 - bg[i] as f32) * k)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        [c(0), c(1), c(2), px[3]]
    };
    let want: Vec<u8> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&px| expect(px))
        .collect();
    let close = out.iter().zip(&want).all(|(a, b)| a.abs_diff(*b) <= 1);
    assert!(close, "stretched {out:?}, expected {want:?}");
    assert_eq!(
        &out[0..4],
        &[20, 30, 40, 255],
        "background stays background"
    );
    let alphas: Vec<u8> = out.as_chunks::<4>().0.iter().map(|px| px[3]).collect();
    assert_eq!(alphas, [255, 128, 7], "alpha is untouched");
}

/// NaN reads as 0 and a negative value as 0, so neither can claim the peak
/// and switch the stretch off: the 0.25 texel still reaches full.
#[test]
fn nan_and_negative_float_texels_read_as_zero() {
    let (bytes, w) = float(&[f32::NAN, -0.5, 0.25, 0.1]);
    let out = stretch(
        wgpu::TextureFormat::Rgba16Float,
        &bytes,
        w,
        TipStretchRefs::COVERAGE,
    );
    let r = reds(&out);
    assert_eq!(r[0], 0, "NaN reads as empty");
    assert_eq!(r[1], 0, "a negative value reads as empty");
    assert_eq!(r[2], 255, "the strongest real texel reaches full");
    assert!(
        r[3].abs_diff(102) <= 1,
        "0.1 scales by the same 4x; got {}",
        r[3]
    );
}

/// Infinity and values above 1 read as 1, exactly as an 8-bit target would
/// have stored them: the dab is already at full, so nothing scales.
#[test]
fn out_of_range_float_texels_saturate() {
    let (bytes, w) = float(&[f32::INFINITY, 1.5, 0.25]);
    let out = stretch(
        wgpu::TextureFormat::Rgba16Float,
        &bytes,
        w,
        TipStretchRefs::COVERAGE,
    );
    let r = reds(&out);
    assert_eq!(&r[..2], &[255, 255]);
    assert!(
        r[2].abs_diff(64) <= 1,
        "0.25 is drawn as rendered; got {}",
        r[2]
    );
}
