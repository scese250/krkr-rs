//! The device composite of a subtree reconstructs the region that changed.
//!
//! The CPU composite is region-limited: it fills the rectangle the change
//! named and draws the tree clipped to it, so a layer that moves inside the
//! subtree costs the rectangle and not the plane. The device composite draws
//! the tree on the device and reads the result back, so the region has to
//! bound the draw, the read-back and the upload as well — otherwise the
//! same move costs the whole plane on the device and the rectangle on the
//! CPU.
//!
//! The reference reconstructs the changed rectangle alone
//! (`reference/krkr2/cpp/core/visual/LayerIntf.cpp:7734-7748`).

use std::collections::HashMap;
use std::time::Instant;

use api::geometry::PixelRegion;
use engine::layer_ops::composite_subtree_on_device;
use engine::{BitmapState, Rect, Scene};
use render::{
    PipelineConfig, RenderTargetDesc, Texture, TextureDesc, begin_offscreen_render_state,
    create_render_pipeline, create_render_target, create_texture, upload_texture,
};

/// The size of the composite plane.
const SIZE: (u32, u32) = (640, 480);

/// The size of the sprite that moves inside the plane.
const SPRITE: (u32, u32) = (64, 64);

/// A solid image of `size` pixels in `colour`.
fn solid(size: (u32, u32), colour: [u8; 4]) -> BitmapState {
    let mut rgba = Vec::with_capacity((size.0 * size.1 * 4) as usize);
    for _ in 0..size.0 * size.1 {
        rgba.extend_from_slice(&colour);
    }
    BitmapState::new(size.0, size.1, rgba)
}

/// The scene: a plane with a sprite on it, the shape a transition's endpoint
/// has — a root layer that holds the screen and a child that draws over it.
///
/// The root holds the transition's consumer count, so it keeps the region
/// that names the part of its content a change makes stale.
fn scene_with_a_sprite() -> (Scene, u32, u32, u32, u32) {
    let mut scene = Scene::default();
    let window = scene.add_window("region", SIZE);
    let root = scene.add_layer(window, None);
    let child = scene.add_layer(window, Some(root));
    let root_image = scene.add_bitmap(SIZE.0, SIZE.1, solid(SIZE, [40, 40, 40, 255]).rgba);
    let child_image = scene.add_bitmap(SPRITE.0, SPRITE.1, solid(SPRITE, [200, 120, 60, 255]).rgba);
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
            state.rect = Rect::new(0, 0, SPRITE.0, SPRITE.1);
            state.visible = true;
            state.bitmap = Some(child_image);
            true
        })
        .expect("the child layer is updated");
    // The transition holds the count for as long as it runs, so the root
    // keeps the region that names the part of its content a change makes
    // stale.
    scene.inc_cache_enabled_count(root);
    (scene, root, child, root_image, child_image)
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

/// The device composite reconstructs the region that changed and nothing
/// else.
///
/// The content holds the whole picture. A layer that moves inside the
/// subtree marks the rectangle it left and the rectangle it reached, and the
/// composite reconstructs that rectangle alone: the upload sends the
/// rectangle's pixels alone, and the content outside it is not stale.
#[test]
fn the_device_composite_reconstructs_the_region_alone() {
    let pipeline =
        create_render_pipeline(PipelineConfig::null()).expect("the null pipeline is created");
    let mut state =
        begin_offscreen_render_state(&pipeline).expect("the offscreen state is created");
    let (mut scene, root, child, root_image, child_image) = scene_with_a_sprite();
    let textures = source_textures(&pipeline, &mut state, &scene, &[root_image, child_image]);
    let target = create_render_target(&pipeline, RenderTargetDesc::new(SIZE.0, SIZE.1))
        .expect("the surface is created");

    // The first composite writes the whole plane: the content is new.
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
    // The frame uploads the content it wrote, so the content is current and
    // the next change starts a new dirty region.
    scene.set_bitmap_clean(content);

    // The sprite moves: the region names the rectangle it left and the one
    // it reached.
    scene
        .update_layer(child, |state| {
            state.rect.x = 32;
            true
        })
        .expect("the sprite moves");
    let region = scene
        .layer_content_region(root)
        .expect("the root keeps the region the move named");

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

    let bitmap = scene.bitmap(content).expect("the content bitmap");
    assert_eq!(
        bitmap.dirty_region,
        Some(PixelRegion::new(
            region.x as u32,
            region.y as u32,
            region.w,
            region.h
        )),
        "the upload sends the region the move named alone: the content's dirty \
         region was {:?}",
        bitmap.dirty_region
    );
}

/// The cost of the composite when the whole plane is stale and when only a
/// rectangle of it is.
///
/// The two numbers are the cost of the same picture: the whole-plane
/// composite redraws the tree, reads the whole surface back and marks the
/// whole content stale; the region composite does the same work over the
/// rectangle the change named.
#[test]
fn the_device_composite_costs_the_region_it_reconstructs() {
    let Some((pipeline, mut state)) = vulkan_device() else {
        eprintln!("the host has no Vulkan device: the cost is not measured");
        return;
    };
    let (mut scene, root, child, root_image, child_image) = scene_with_a_sprite();
    let textures = source_textures(&pipeline, &mut state, &scene, &[root_image, child_image]);
    let target = create_render_target(&pipeline, RenderTargetDesc::new(SIZE.0, SIZE.1))
        .expect("the surface is created");

    // The first composite writes the whole plane.
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
    // The frame uploads the content it wrote, so the next change starts a
    // new dirty region.
    scene.set_bitmap_clean(content);

    // The whole plane is stale: the change named all of it.
    scene.invalidate_layer_content(root);
    let whole = measure(&mut scene, root, &pipeline, &mut state, &target, &textures);

    // The sprite moves: only a rectangle of the plane is stale.
    scene
        .update_layer(child, |state| {
            state.rect.x = 32;
            true
        })
        .expect("the sprite moves");
    // The frame uploads the content it wrote, so the next change starts a
    // new dirty region.
    if let Some(content) = scene.subtree_content_id(root) {
        scene.set_bitmap_clean(content);
    }
    let region = measure(&mut scene, root, &pipeline, &mut state, &target, &textures);

    println!(
        "the device composite: the whole plane {:.2} ms, the region {:.2} ms",
        whole.0, region.0
    );
    println!(
        "the content's dirty region: the whole plane {:?}, the region {:?}",
        whole.1, region.1
    );
    assert!(
        region.0 < whole.0,
        "the region composite is cheaper than the whole-plane one: {region:?}"
    );
}

/// The Vulkan pipeline and its offscreen state, or `None` when the host has
/// no device.
fn vulkan_device() -> Option<(render::RenderPipeline, render::RenderState)> {
    let pipeline = create_render_pipeline(PipelineConfig::vulkan()).ok()?;
    let state = begin_offscreen_render_state(&pipeline).ok()?;
    Some((pipeline, state))
}

/// The composite's cost and the content's dirty region.
type Measured = (f64, Option<PixelRegion>);

/// Runs one composite and reports its cost and the content's dirty region.
fn measure(
    scene: &mut Scene,
    root: u32,
    pipeline: &render::RenderPipeline,
    state: &mut render::RenderState,
    target: &render::RenderTarget,
    textures: &HashMap<u32, Texture>,
) -> Measured {
    let started = Instant::now();
    let content = composite_subtree_on_device(scene, root, pipeline, state, target, &mut |id| {
        textures.get(&id).cloned()
    })
    .expect("the device composite runs")
    .expect("the device composite produced a content");
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let region = scene
        .bitmap(content)
        .expect("the content bitmap")
        .dirty_region;
    (elapsed, region)
}
