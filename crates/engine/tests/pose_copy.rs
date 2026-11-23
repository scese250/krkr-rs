//! The pose's copy from one layer to another, on the device.
//!
//! The pose lives on the backend: the layer's image keeps the content the
//! pose is drawn over, and the pose itself is on a surface of its own. A
//! layer that takes over another's pose — `Layer.assignImages` does exactly
//! that, every frame, in the games that animate a title — must copy the pose
//! where it lives: on the device. The copy never brings a whole canvas
//! through the host, which is what the read-back seam costs.
//!
//! The tests below compare the two paths on the Vulkan backend: the device
//! copy against the read-back, which is the oracle for what a script sees.

use std::sync::Arc;

use engine::layer_ops::{DrawOp, ImageDraw};
use engine::{BitmapState, Scene};
use render::{
    PipelineConfig, RenderPipeline, RenderState, begin_offscreen_render_state,
    create_render_pipeline,
};

/// The width and height of the layer the pose is drawn into.
const TARGET: (u32, u32) = (64, 48);

/// The Vulkan pipeline and its render state, or `None` when the host has no
/// device.
///
/// The copy is the device's work, so the test runs on the Vulkan backend. A
/// host without a device skips it: the null backend is the CPU fallback, and
/// it is not what this file is about.
fn vulkan_device() -> Option<(RenderPipeline, RenderState)> {
    let pipeline = create_render_pipeline(PipelineConfig::vulkan()).ok()?;
    let state = begin_offscreen_render_state(&pipeline).ok()?;
    Some((pipeline, state))
}

/// A source of `size` pixels whose colour is uniform.
fn uniform_source(size: u32, colour: [u8; 4]) -> BitmapState {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..(size * size) {
        rgba.extend_from_slice(&colour);
    }
    BitmapState::new(size, size, rgba)
}

/// A scene with two layers whose images are the target, and their ids.
///
/// The second layer is the one that takes over the first layer's pose.
fn scene_with_two_targets() -> (Scene, u32, u32, u32, u32) {
    let mut scene = Scene::default();
    let window = scene.add_window("pose", TARGET);
    let source = scene.add_layer(window, None);
    let destination = scene.add_layer(window, None);
    let source_bitmap = scene.add_bitmap(
        TARGET.0,
        TARGET.1,
        vec![0u8; (TARGET.0 * TARGET.1 * 4) as usize],
    );
    let destination_bitmap = scene.add_bitmap(
        TARGET.0,
        TARGET.1,
        vec![0u8; (TARGET.0 * TARGET.1 * 4) as usize],
    );
    if let Some(layer) = scene.layer_mut(source) {
        layer.bitmap = Some(source_bitmap);
    }
    if let Some(layer) = scene.layer_mut(destination) {
        layer.bitmap = Some(destination_bitmap);
    }
    (
        scene,
        source,
        source_bitmap,
        destination,
        destination_bitmap,
    )
}

/// The draw list: one placement of a source over the target.
fn draw_list(source: u32) -> Vec<DrawOp> {
    vec![DrawOp::Image(ImageDraw {
        source,
        rect: (0, 0, 16, 16),
        corners: [(4.0, 4.0), (44.0, 4.0), (4.0, 36.0)],
        opacity: 255,
        mode: 2,
        source_over: true,
    })]
}

/// The pixels a script reads for the bitmap: the image's pixels with the
/// pose composited over them.
fn pixels(
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

/// The copy puts the source's pose on the destination, and the destination's
/// own image keeps its content.
#[test]
fn the_copied_pose_reaches_the_destination() {
    let (mut scene, source, source_bitmap, _destination, destination_bitmap) =
        scene_with_two_targets();
    let draw_source = scene.add_draw_source(Arc::new(uniform_source(16, [200, 120, 60, 255])));

    let Some((pipeline, mut state)) = vulkan_device() else {
        eprintln!("the host has no Vulkan device: the device copy is not checked");
        return;
    };
    scene.set_layer_draws(source, draw_list(draw_source));
    scene
        .draw_layer_draws_gpu(&pipeline, &mut state, source)
        .expect("the pose is drawn");

    scene
        .copy_pose_on_device(&pipeline, &mut state, source_bitmap, destination_bitmap)
        .expect("the pose is copied");

    // The destination shows the pose the source showed: the pixels a script
    // reads there carry it.
    let copied = pixels(&mut scene, &pipeline, &mut state, destination_bitmap);
    let original = pixels(&mut scene, &pipeline, &mut state, source_bitmap);
    assert!(
        copied
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| *pixel != [0, 0, 0, 0]),
        "the copied pose reaches the destination's pixels"
    );
    assert_eq!(
        copied, original,
        "the destination shows the same picture the source showed"
    );
    // The destination's own image is untouched: the pose is a picture of its
    // own, and the copy moves the pose, not the image.
    let image = scene.bitmap(destination_bitmap).expect("the bitmap");
    assert!(
        image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 0, 0, 0]),
        "the destination's image keeps its own content"
    );
}

/// A source with no pose leaves the destination with none: the destination's
/// own pose is not the source's.
#[test]
fn a_source_with_no_pose_leaves_the_destination_with_none() {
    let (mut scene, _source, source_bitmap, _destination, destination_bitmap) =
        scene_with_two_targets();

    let Some((pipeline, mut state)) = vulkan_device() else {
        eprintln!("the host has no Vulkan device: the device copy is not checked");
        return;
    };
    scene
        .copy_pose_on_device(&pipeline, &mut state, source_bitmap, destination_bitmap)
        .expect("the copy is made");
    assert!(
        !scene.has_pose(destination_bitmap),
        "the destination shows no pose: the source showed none"
    );
}
