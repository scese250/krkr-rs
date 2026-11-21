//! The device composite of a layer's subtree against the CPU composite.
//!
//! The CPU composite is the oracle: it blends every draw of the tree pixel
//! by pixel on the CPU. The device composite draws the same tree on the
//! device and reads the result back, so the two agree everywhere but on the
//! blend's rounding, and this file says how far they may differ.
//!
//! The device path runs on the null backend, whose rasterizer is the
//! pipeline's own software renderer, so the comparison needs no device. The
//! Vulkan backend composites in the premultiplied space and holds the
//! premultiplied form, which the engine brings back into the straight form
//! the content bitmap holds; that round trip is one level at most.

use std::collections::HashMap;

use api::geometry::PixelRegion;
use engine::layer_ops::{composite_subtree_on_device, render_layer_subtree};
use engine::{BitmapState, Rect, Scene};
use render::{
    PipelineConfig, RenderTargetDesc, Texture, TextureDesc, begin_offscreen_render_state,
    create_render_pipeline, create_render_target, create_texture, upload_texture,
};

/// The size of the window and of the root layer's image.
const SIZE: (u32, u32) = (16, 12);

/// The rectangle the child layer covers.
fn child_rect() -> Rect {
    Rect::new(4, 2, 8, 8)
}

/// A solid image of `size` pixels in `colour`.
fn solid(size: (u32, u32), colour: [u8; 4]) -> BitmapState {
    let mut rgba = Vec::with_capacity((size.0 * size.1 * 4) as usize);
    for _ in 0..size.0 * size.1 {
        rgba.extend_from_slice(&colour);
    }
    BitmapState::new(size.0, size.1, rgba)
}

/// A scene with a root layer that has an image and a child layer with one of
/// its own, so the subtree is not trivial and the composite has two draws.
///
/// The root's image is half transparent, so the child's pixels are composited
/// over a transparent background and the two draws have to agree about the
/// coverage they leave.
fn scene_with_subtree() -> (Scene, u32, u32, u32) {
    let mut scene = Scene::default();
    let window = scene.add_window("device", SIZE);
    let root = scene.add_layer(window, None);
    let child = scene.add_layer(window, Some(root));
    let root_image = scene.add_bitmap(SIZE.0, SIZE.1, solid(SIZE, [200, 120, 60, 128]).rgba);
    let rect = child_rect();
    let child_image = scene.add_bitmap(
        rect.w,
        rect.h,
        solid((rect.w, rect.h), [10, 220, 30, 200]).rgba,
    );
    scene
        .update_layer(root, |state| {
            state.rect = Rect::new(0, 0, SIZE.0, SIZE.1);
            state.visible = true;
            state.bitmap = Some(root_image);
            true
        })
        .expect("the root layer is updated");
    scene
        .update_layer(child, |state| {
            state.rect = child_rect();
            state.visible = true;
            state.bitmap = Some(child_image);
            true
        })
        .expect("the child layer is updated");
    (scene, root, root_image, child_image)
}

/// The textures the device samples the sources from, uploaded.
fn source_textures(
    pipeline: &render::RenderPipeline,
    state: &mut render::RenderState,
    scene: &Scene,
    images: &[u32],
) -> HashMap<u32, Texture> {
    let mut textures = HashMap::new();
    for id in images {
        let Some(bitmap) = scene.bitmap(*id) else {
            continue;
        };
        let texture = create_texture(pipeline, TextureDesc::new(bitmap.width, bitmap.height))
            .expect("the source's texture is created");
        upload_texture(
            pipeline,
            state,
            &texture,
            PixelRegion::new(0, 0, bitmap.width, bitmap.height),
            &bitmap.rgba,
        )
        .expect("the source is uploaded");
        textures.insert(*id, texture);
    }
    textures
}

/// The largest difference between two pixels' channels.
fn channel_gap(left: &[u8], right: &[u8]) -> u32 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| u32::from(a.abs_diff(*b)))
        .max()
        .unwrap_or(0)
}

/// The device composite draws the picture the CPU composite draws.
///
/// The two paths differ by the blend's rounding: the CPU path works in the
/// byte space and the device in floating point, so a channel can be one
/// level apart.
#[test]
fn the_device_composite_draws_the_picture_the_cpu_composite_draws() {
    let (mut cpu, root, _, _) = scene_with_subtree();
    let cpu_content = render_layer_subtree(&mut cpu, root).expect("the CPU composite");
    let cpu_pixels = cpu
        .bitmap(cpu_content)
        .expect("the content bitmap")
        .rgba
        .clone();

    let pipeline =
        create_render_pipeline(PipelineConfig::null()).expect("the null pipeline is created");
    let mut state =
        begin_offscreen_render_state(&pipeline).expect("the offscreen state is created");
    let (mut scene, root, root_image, child_image) = scene_with_subtree();
    let textures = source_textures(&pipeline, &mut state, &scene, &[root_image, child_image]);
    let target = create_render_target(&pipeline, RenderTargetDesc::new(SIZE.0, SIZE.1))
        .expect("the surface is created");
    let content = composite_subtree_on_device(
        &mut scene,
        root,
        &pipeline,
        &mut state,
        &target,
        &mut |id| textures.get(&id).cloned(),
    )
    .expect("the device composite runs")
    .expect("the device composite produced a content");
    let device_pixels = scene
        .bitmap(content)
        .expect("the content bitmap")
        .rgba
        .clone();

    assert_eq!(
        device_pixels.len(),
        cpu_pixels.len(),
        "the content holds the same pixels"
    );
    let gap = channel_gap(&cpu_pixels, &device_pixels);
    assert!(
        gap <= 2,
        "the two composites agree within one level: the worst gap was {gap}"
    );
}

/// The device composite writes the whole content, so the region that named
/// the missing part is settled: the next composite finds the content current
/// and draws nothing.
#[test]
fn the_device_composite_settles_the_content_region() {
    let pipeline =
        create_render_pipeline(PipelineConfig::null()).expect("the null pipeline is created");
    let mut state =
        begin_offscreen_render_state(&pipeline).expect("the offscreen state is created");
    let (mut scene, root, root_image, child_image) = scene_with_subtree();
    let textures = source_textures(&pipeline, &mut state, &scene, &[root_image, child_image]);
    let target = create_render_target(&pipeline, RenderTargetDesc::new(SIZE.0, SIZE.1))
        .expect("the surface is created");

    composite_subtree_on_device(
        &mut scene,
        root,
        &pipeline,
        &mut state,
        &target,
        &mut |id| textures.get(&id).cloned(),
    )
    .expect("the device composite runs")
    .expect("the device composite produced a content");
    assert!(
        scene.layer_content_region(root).is_none(),
        "the content is current: the region that named the missing part is settled"
    );
}

/// A tree the device cannot draw is refused, and the refusal is a typed
/// error: the caller falls back to the CPU composite.
///
/// A mask face derives its coverage plane from the layer's own image, and
/// the plane has no bitmap for the device to sample.
#[test]
fn a_tree_the_device_cannot_draw_is_refused() {
    let pipeline =
        create_render_pipeline(PipelineConfig::null()).expect("the null pipeline is created");
    let mut state =
        begin_offscreen_render_state(&pipeline).expect("the offscreen state is created");
    let (mut scene, root, root_image, child_image) = scene_with_subtree();
    let textures = source_textures(&pipeline, &mut state, &scene, &[root_image, child_image]);
    let target = create_render_target(&pipeline, RenderTargetDesc::new(SIZE.0, SIZE.1))
        .expect("the surface is created");
    scene
        .update_layer(root, |state| {
            state.face = 2;
            true
        })
        .expect("the root layer is updated");

    let error = composite_subtree_on_device(
        &mut scene,
        root,
        &pipeline,
        &mut state,
        &target,
        &mut |id| textures.get(&id).cloned(),
    )
    .expect_err("a coverage plane is refused");
    assert!(
        matches!(error, api::error::RenderError::Unsupported { .. }),
        "the refusal is a typed error: {error}"
    );
}
