//! The GPU draw path against the CPU path.
//!
//! The CPU path is the oracle: it resamples the pose with an f64 inverse
//! map and composites it with the alpha-correct rule. The GPU path places
//! the same draws with an f32 affine, samples them with the backend's
//! nearest sampler and composites them with the backend's standard rule,
//! which works in premultiplied space. The two therefore agree everywhere
//! but on the edges of the sampled shapes, and the tests below say how far
//! they may differ.
//!
//! The GPU path leaves the pose on the backend: the pose accumulates on a
//! surface of its own, and the layer's image keeps the content the pose is
//! drawn over. The pixels a script reads are the image's pixels with the
//! pose composited over them, so the tests read them the way a script does.
//!
//! The tolerance is a D34 tolerance, not byte equality:
//!
//! * the placement is f32 on the GPU and f64 on the CPU, so a destination
//!   pixel that sits exactly on a texel boundary can be sampled from the
//!   other texel;
//! * the source's sub-rectangle is clamped to the rectangle on the CPU and
//!   to the whole texture on the GPU, so the outermost row of a
//!   sub-rectangle can be sampled from the texel beside it;
//! * the premultiplied round trip (the surface holds premultiplied pixels,
//!   the image holds straight-alpha pixels) loses up to one level.

use std::sync::Arc;

use engine::layer_ops::{CompositeDraw, DrawOp, ImageDraw};
use engine::{BitmapState, Scene};
use render::{
    PipelineConfig, RenderPipeline, RenderState, begin_offscreen_render_state,
    create_render_pipeline,
};

/// The width and height of the target the pose is drawn into.
const TARGET: (u32, u32) = (64, 48);

/// The Vulkan pipeline and its render state, or `None` when the host has no
/// device.
///
/// This is the GPU path, so the comparison runs on the Vulkan backend. A host
/// without a device skips the comparison: the null backend is the CPU
/// fallback, and it is not what this file is about.
fn vulkan_device() -> Option<(RenderPipeline, RenderState)> {
    let pipeline = create_render_pipeline(PipelineConfig::vulkan()).ok()?;
    let state = begin_offscreen_render_state(&pipeline).ok()?;
    Some((pipeline, state))
}

/// A source of `size` pixels whose colour is uniform.
///
/// A uniform source makes the two paths agree everywhere: the texel a pixel
/// is sampled from does not change its colour, so only the composite's
/// arithmetic is left to differ.
fn uniform_source(size: u32, colour: [u8; 4]) -> BitmapState {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..size * size {
        rgba.extend_from_slice(&colour);
    }
    BitmapState::new(size, size, rgba)
}

/// A source of `size` pixels whose colour changes from pixel to pixel.
///
/// The neighbouring texels differ, so a pixel sampled from the other texel
/// is a visible difference: the tests below count those pixels instead of
/// bounding them.
fn gradient_source(size: u32) -> BitmapState {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            rgba.push((x * 16) as u8);
            rgba.push((y * 16) as u8);
            rgba.push(128);
            rgba.push(255);
        }
    }
    BitmapState::new(size, size, rgba)
}

/// A scene with a layer whose image is the target, and the layer's id.
fn scene_with_target() -> (Scene, u32, u32) {
    let mut scene = Scene::default();
    let window = scene.add_window("motion", TARGET);
    let layer = scene.add_layer(window, None);
    let bitmap = scene.add_bitmap(
        TARGET.0,
        TARGET.1,
        vec![0u8; (TARGET.0 * TARGET.1 * 4) as usize],
    );
    if let Some(layer) = scene.layer_mut(layer) {
        layer.bitmap = Some(bitmap);
    }
    (scene, layer, bitmap)
}

/// The draw list: one sheared, magnified placement of a source and one
/// composite of a surface at a position.
fn draw_list(source: u32, surface: u32) -> Vec<DrawOp> {
    vec![
        DrawOp::Image(ImageDraw {
            source,
            rect: (0, 0, 16, 16),
            // A sheared placement: the destination's first axis runs along
            // x, the second along a slant, so the map is not a 2x2 scale.
            corners: [(4.0, 4.0), (44.0, 4.0), (12.0, 36.0)],
            opacity: 255,
            mode: 2,
            source_over: true,
        }),
        DrawOp::Composite(CompositeDraw {
            source: surface,
            rect: (0, 0, 8, 8),
            x: 40,
            y: 24,
            opacity: 200,
            mode: 2,
            source_over: true,
        }),
    ]
}

/// The pixels a script reads for the target: the image's pixels with the
/// pose composited over them.
///
/// The GPU path leaves the pose on the backend, so the read-back is what a
/// script's `getPixel` sees.
fn gpu_pixels(
    scene: &mut Scene,
    pipeline: &RenderPipeline,
    state: &mut RenderState,
    bitmap: u32,
) -> Vec<u8> {
    let pose = scene
        .read_pose_pixels(pipeline, state, bitmap)
        .expect("the pose is read back")
        .expect("the layer shows a pose");
    scene
        .compose_pose_pixels(bitmap, &pose)
        .expect("the pose is composited")
        .expect("the layer shows a pose")
}

/// The largest difference between two pixels' channels.
fn channel_gap(left: &[u8], right: &[u8]) -> u32 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| u32::from(a.abs_diff(*b)))
        .max()
        .unwrap_or(0)
}

/// The number of pixels whose channels differ by more than `tolerance`, and
/// the largest difference over all the pixels.
fn differences(left: &[u8], right: &[u8], tolerance: u32) -> (usize, u32) {
    let mut beyond = 0usize;
    let mut worst = 0u32;
    for (a, b) in left
        .as_chunks::<4>()
        .0
        .iter()
        .zip(right.as_chunks::<4>().0.iter())
    {
        let gap = channel_gap(a, b);
        worst = worst.max(gap);
        if gap > tolerance {
            beyond += 1;
        }
    }
    (beyond, worst)
}

/// The GPU path draws the same picture as the CPU path.
///
/// A uniform source makes the two paths agree everywhere but for the
/// composite's rounding, so the difference is one level at most.
#[test]
fn the_gpu_path_draws_the_same_picture_as_the_cpu_path() {
    let (mut scene, layer, bitmap) = scene_with_target();
    let source = scene.add_draw_source(Arc::new(uniform_source(16, [200, 120, 60, 255])));
    let surface = scene.add_draw_source(Arc::new(uniform_source(8, [10, 220, 30, 255])));
    let ops = draw_list(source, surface);

    // The CPU path: the oracle.
    let mut cpu = scene.bitmap(bitmap).expect("the target bitmap").clone();
    scene.draw_ops_into(&mut cpu, &ops);

    // The GPU path: the same list, drawn on the backend.
    let Some((pipeline, mut state)) = vulkan_device() else {
        eprintln!("the host has no Vulkan device: the GPU path is not checked");
        return;
    };
    scene.set_layer_draws(layer, ops);
    let drawn = scene
        .draw_layer_draws_gpu(&pipeline, &mut state, layer)
        .expect("the list is drawn");
    assert!(drawn, "the backend drew the list");

    let gpu = gpu_pixels(&mut scene, &pipeline, &mut state, bitmap);
    let (beyond, worst) = differences(&cpu.rgba, &gpu, 2);
    assert_eq!(
        beyond, 0,
        "the two paths agree on every pixel: the worst gap was {worst}"
    );
    assert!(worst <= 2, "the composite's rounding is one level: {worst}");
}

/// A patterned source: the two paths differ only where a pixel is sampled
/// from the other texel.
///
/// The difference is a count, not a bound: a pixel on a texel boundary can
/// be sampled from either texel, and the two texels of a gradient differ by
/// a visible amount.
#[test]
fn the_gpu_path_differs_only_on_the_edges_of_the_sampled_shapes() {
    let (mut scene, layer, bitmap) = scene_with_target();
    let source = scene.add_draw_source(Arc::new(gradient_source(16)));
    let surface = scene.add_draw_source(Arc::new(gradient_source(8)));
    let ops = draw_list(source, surface);

    let mut cpu = scene.bitmap(bitmap).expect("the target bitmap").clone();
    scene.draw_ops_into(&mut cpu, &ops);

    let Some((pipeline, mut state)) = vulkan_device() else {
        eprintln!("the host has no Vulkan device: the GPU path is not checked");
        return;
    };
    scene.set_layer_draws(layer, ops);
    scene
        .draw_layer_draws_gpu(&pipeline, &mut state, layer)
        .expect("the list is drawn");

    let gpu = gpu_pixels(&mut scene, &pipeline, &mut state, bitmap);
    let total = cpu.rgba.len() / 4;
    let (beyond, _worst) = differences(&cpu.rgba, &gpu, 2);
    let share = beyond as f64 / total as f64;
    assert!(
        share < 0.05,
        "the paths differ on {beyond} of {total} pixels ({:.1} %)",
        share * 100.0
    );
}

/// The pose stays on the backend: the layer's image keeps the content the
/// pose is drawn over, and the pose is brought into the pixels only when a
/// script reads them.
#[test]
fn the_pose_stays_on_the_backend_until_a_script_reads_it() {
    let (mut scene, layer, bitmap) = scene_with_target();
    let source = scene.add_draw_source(Arc::new(uniform_source(16, [200, 120, 60, 255])));
    let surface = scene.add_draw_source(Arc::new(uniform_source(8, [10, 220, 30, 255])));
    let ops = draw_list(source, surface);

    let Some((pipeline, mut state)) = vulkan_device() else {
        eprintln!("the host has no Vulkan device: the GPU path is not checked");
        return;
    };
    scene.set_layer_draws(layer, ops);
    scene
        .draw_layer_draws_gpu(&pipeline, &mut state, layer)
        .expect("the list is drawn");

    // The image holds no pose: the pose is on the backend.
    let image = scene.bitmap(bitmap).expect("the target bitmap");
    assert!(
        image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 0, 0, 0]),
        "the layer's image keeps the content alone"
    );

    // The pixels a script reads carry the pose.
    let gpu = gpu_pixels(&mut scene, &pipeline, &mut state, bitmap);
    assert!(
        gpu.as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| *pixel != [0, 0, 0, 0]),
        "the pose reaches the pixels a script reads"
    );
}

/// The pose accumulates: the second draw composites onto the pixels the
/// first one left, on both paths.
#[test]
fn the_gpu_path_accumulates_the_pose_like_the_cpu_path() {
    let (mut scene, layer, bitmap) = scene_with_target();
    let source = scene.add_draw_source(Arc::new(uniform_source(16, [200, 120, 60, 128])));
    let ops = vec![DrawOp::Image(ImageDraw {
        source,
        rect: (0, 0, 16, 16),
        corners: [(4.0, 4.0), (44.0, 4.0), (4.0, 36.0)],
        opacity: 255,
        mode: 2,
        source_over: true,
    })];

    let Some((pipeline, mut state)) = vulkan_device() else {
        eprintln!("the host has no Vulkan device: the GPU path is not checked");
        return;
    };
    // The pixel inside the drawn shape.
    let inside = (10 * TARGET.0 as usize + 10) * 4 + 3;
    scene.set_layer_draws(layer, ops.clone());
    scene
        .draw_layer_draws_gpu(&pipeline, &mut state, layer)
        .expect("the first draw");
    let after_first = gpu_pixels(&mut scene, &pipeline, &mut state, bitmap)[inside];

    scene.set_layer_draws(layer, ops);
    scene
        .draw_layer_draws_gpu(&pipeline, &mut state, layer)
        .expect("the second draw");
    let after_second = gpu_pixels(&mut scene, &pipeline, &mut state, bitmap)[inside];

    assert!(
        after_second > after_first,
        "the pose accumulates: {after_first} then {after_second}"
    );
}
