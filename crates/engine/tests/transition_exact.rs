//! The one description of a transition: the cross-fade pass and the exact
//! CPU fallback.
//!
//! A transition is drawn in one of two ways: the GPU pass the reference's
//! blend is a program for, and the CPU blend [`Trans::apply`] writes. Both
//! use the reference's formula, `s1 + (s2 - s1) * t`. The tree carries no
//! third description of a transition: a transition that is neither a pass nor
//! a CPU blend does not exist.
//!
//! The tests below pin the pass's shape (what it binds and the weight it
//! carries), the CPU blend's arithmetic (the reference's formula, computed
//! here from the reference's own expression), and the projection's rules for
//! a running transition: it is drawn by its pass alone — one node, no cached
//! subtree, the children live or held — and it follows the layer's image
//! placement.

use api::blend::BlendMode;
use api::geometry::{PixelRegion, RectF};
use engine::shader::builtin;
use engine::shader::{EffectDraw, Program};
use engine::trans::{DestAlpha, Trans};
use engine::{BitmapState, Map, Rect, Scene, project_window_report};
use render::{
    EffectPipeline, PipelineConfig, RenderContent, RenderPipeline, RenderState, Surface, Texture,
    TextureDesc, begin_render_state, create_effect_pipeline, create_render_pipeline,
    create_texture, upload_texture,
};
use testkit::headless_window;

/// A bitmap of one colour.
fn solid(width: u32, height: u32, colour: [u8; 4]) -> BitmapState {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        rgba.extend_from_slice(&colour);
    }
    BitmapState::new(width, height, rgba)
}

/// One channel of the reference's constant-alpha blend.
///
/// The reference's byte-space blend is `s1 + ((s2 - s1) * t) >> 8`
/// (`reference/cpp/core/visual/TransIntf.cpp:592-600`).
fn blend_channel(s1: u8, s2: u8, t: i32) -> u8 {
    let s1 = i32::from(s1);
    let s2 = i32::from(s2);
    (s1 + (((s2 - s1) * t) >> 8)).clamp(0, 255) as u8
}

/// The share of a destination a source covers, in the alpha-vs-alpha mix.
///
/// The source's share is the ratio of its opacity to the sum of it and the
/// destination's opacity the source does not hide
/// (`reference/cpp/core/visual/ogl/RenderManager_ogl.cpp:3278-3291`).
fn coverage(source_alpha: u32, destination_alpha: u32) -> i32 {
    let shown = f64::from(source_alpha) / 255.0;
    let hidden = f64::from(destination_alpha) / 255.0;
    let share = shown / (hidden * (1.0 - shown) + shown + 0.0001);
    ((share * 255.0) as i32).clamp(0, 255)
}

/// A cross-fade is one pass over the two endpoints, and the pass carries the
/// phase as the blend weight.
///
/// The reference blends the two endpoints with `s1 + (s2 - s1) * t`, where
/// `t` is the phase over 256 (`reference/cpp/core/visual/TransIntf.cpp:592-600`,
/// `reference/cpp/core/visual/ogl/RenderManager_ogl.cpp:3260-3270`).
#[test]
fn a_cross_fade_is_one_pass_over_the_two_endpoints() {
    let mut trans = Trans::cross_fade(100.0).expect("cross-fade");
    trans.seek_ms(50.0);
    let phase = trans.phase();
    assert!(
        phase > 0 && phase < trans.phase_max(),
        "the phase is mid-transition: {phase}"
    );

    let draw = trans
        .effect_draw(11, 22, (4, 4), true)
        .expect("the cross-fade draws");
    assert_eq!(draw.passes.len(), 1, "one blend covers the frame");
    assert_eq!(draw.size, (4, 4));
    assert!(draw.covers_layer);

    let pass = &draw.passes[0];
    assert_eq!(pass.program, Program::CrossFade);
    assert_eq!(pass.textures.len(), 2, "the blend reads the two endpoints");
    assert_eq!(pass.textures[0].bitmap, 11);
    assert_eq!(pass.textures[1].bitmap, 22);
    assert_eq!(
        pass.dst,
        Rect::new(0, 0, 4, 4),
        "the blend covers the frame"
    );

    // The weight is the phase over 256: the reference's `t`.
    let words = pass.uniforms.words();
    assert_eq!(
        words[builtin::PARAM_UNIFORM as usize],
        phase as f32 / 256.0,
        "the phase reaches the program as the blend weight"
    );
}

/// The CPU fallback is the reference's blend: `s1 + (s2 - s1) * t` per
/// channel, and the endpoints are copied verbatim.
///
/// The reference's constant-alpha blend is `mix(s, d, opacity)`
/// (`reference/cpp/core/visual/ogl/RenderManager_ogl.cpp:3260-3270`), and its
/// byte-space form is the expression below.
#[test]
fn the_cpu_blend_is_the_reference_formula() {
    let first = solid(4, 4, [10, 40, 90, 255]);
    let second = solid(4, 4, [200, 120, 30, 255]);
    let mut trans = Trans::cross_fade(100.0).expect("cross-fade");
    // A destination that ignores its alpha mixes the four channels alike
    // (`reference/cpp/core/visual/TransIntf.cpp:682-686`).
    trans.dest_alpha = DestAlpha::Opaque;
    trans.seek_ms(50.0);
    let phase = trans.phase();

    let mut dst = solid(4, 4, [0, 0, 0, 0]);
    trans
        .apply(&mut dst, &first, &second, (0, 0, 4, 4))
        .expect("the blend");
    for (index, pixel) in dst.rgba.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!(pixel[0], blend_channel(10, 200, phase), "pixel {index}");
        assert_eq!(pixel[1], blend_channel(40, 120, phase), "pixel {index}");
        assert_eq!(pixel[2], blend_channel(90, 30, phase), "pixel {index}");
        assert_eq!(pixel[3], blend_channel(255, 255, phase), "pixel {index}");
    }
}

/// The endpoints are exact copies of one source: the transition's own
/// short-circuit (`reference/cpp/core/visual/TransIntf.cpp:592-600`).
#[test]
fn the_cpu_blend_copies_one_source_at_the_endpoints() {
    let first = solid(2, 1, [10, 40, 90, 255]);
    let second = solid(2, 1, [200, 120, 30, 255]);
    let mut trans = Trans::cross_fade(100.0).expect("cross-fade");
    trans.dest_alpha = DestAlpha::Opaque;

    let mut dst = solid(2, 1, [0, 0, 0, 0]);
    trans
        .apply(&mut dst, &first, &second, (0, 0, 2, 1))
        .expect("the blend");
    assert_eq!(dst.rgba, first.rgba, "the start is source 1 verbatim");

    trans.complete();
    trans
        .apply(&mut dst, &first, &second, (0, 0, 2, 1))
        .expect("the blend");
    assert_eq!(dst.rgba, second.rgba, "the end is source 2 verbatim");
}

/// A destination with a straight alpha per pixel gets the coverage mix: the
/// source covers the destination with the share the two alphas give it.
///
/// The reference picks the mix from the destination's type
/// (`reference/cpp/core/visual/TransIntf.cpp:664-673`), and the coverage mix
/// is the one the GPU program runs
/// (`reference/cpp/core/visual/ogl/RenderManager_ogl.cpp:3278-3291`). The
/// plain composition of the two sources — one source-over the other at the
/// phase — is a different number, so it cannot stand in for the reference's
/// blend.
#[test]
fn the_straight_alpha_blend_is_the_coverage_mix() {
    let first = solid(2, 1, [10, 40, 90, 255]);
    let second = solid(2, 1, [200, 120, 30, 128]);
    let mut trans = Trans::cross_fade(100.0).expect("cross-fade");
    trans.seek_ms(50.0);
    let phase = trans.phase();

    let mut dst = solid(2, 1, [0, 0, 0, 0]);
    trans
        .apply(&mut dst, &first, &second, (0, 0, 2, 1))
        .expect("the blend");

    // The share the two alphas give the source: the source's own opacity
    // scaled by the phase, over the destination's opacity the source does not
    // hide.
    let weight_phase = u32::try_from(phase).expect("the phase is a weight");
    let source = (128u32 * weight_phase) & 0xff00;
    let destination = 255u32 * (256 - weight_phase);
    let weight = coverage(source >> 8, destination >> 8);

    assert_eq!(
        dst.rgba[0],
        blend_channel(10, 200, weight),
        "the red channel"
    );
    assert_eq!(
        dst.rgba[1],
        blend_channel(40, 120, weight),
        "the green channel"
    );
    assert_eq!(
        dst.rgba[2],
        blend_channel(90, 30, weight),
        "the blue channel"
    );
    // The alpha is the two alphas mixed by the phase.
    assert_eq!(
        dst.rgba[3],
        blend_channel(255, 128, phase),
        "the alpha channel"
    );

    // The plain composition of the two sources is a different number: the
    // phase alone is not the share the source covers.
    assert_ne!(
        dst.rgba[0],
        blend_channel(10, 200, phase),
        "the coverage mix is not the plain composition"
    );
}

/// The Vulkan pipeline, its surface and its render state.
struct Device {
    pipeline: RenderPipeline,
    /// Kept alive for the state it belongs to.
    _surface: Surface,
    state: RenderState,
}

/// The device the transition's pass is drawn with, or `None` when the host
/// has neither a device nor a surface.
fn vulkan_device() -> Option<Device> {
    let pipeline = create_render_pipeline(PipelineConfig::vulkan()).ok()?;
    let (surface, state) = begin_render_state(&pipeline, &headless_window()).ok()?;
    Some(Device {
        pipeline,
        _surface: surface,
        state,
    })
}

/// The pipeline of the cross-fade program, created on the device.
fn cross_fade_pipeline(device: &mut Device) -> Option<EffectPipeline> {
    let desc = builtin::crossfade().pipeline_desc(BlendMode::SourceOver);
    create_effect_pipeline(&device.pipeline, &mut device.state, &desc).ok()
}

/// The textures of the scene's bitmaps, uploaded to the device.
fn textures_for(scene: &Scene, device: &mut Device) -> Map<u32, Texture> {
    let mut textures: Map<u32, Texture> = Map::default();
    for bitmap in &scene.bitmaps {
        let Ok(texture) = create_texture(
            &device.pipeline,
            TextureDesc::new(bitmap.width, bitmap.height),
        ) else {
            continue;
        };
        if upload_texture(
            &device.pipeline,
            &mut device.state,
            &texture,
            PixelRegion::new(0, 0, bitmap.width, bitmap.height),
            &bitmap.rgba,
        )
        .is_err()
        {
            continue;
        }
        textures.insert(bitmap.id, texture);
    }
    textures
}

/// A cross-fade over two sources, at the phase that is half the transition.
fn cross_fade_draw(first: u32, second: u32, size: (u32, u32), covers_layer: bool) -> EffectDraw {
    let mut trans = Trans::cross_fade(100.0).expect("cross-fade");
    trans.seek_ms(50.0);
    trans
        .effect_draw(first, second, size, covers_layer)
        .expect("the cross-fade draws")
}

/// A running transition is drawn by its pass alone: one node, no cached
/// subtree, and the children live.
///
/// The reference draws the transition's frame while it composites the layer,
/// so the frame never reaches a cache region
/// (`reference/cpp/core/visual/LayerIntf.cpp:8186-8199`), and a transition
/// that covers only the layer's own image leaves the children live
/// (`reference/cpp/core/visual/LayerIntf.cpp:6745-6768`, `:7156-7157`).
#[test]
fn a_running_transition_is_drawn_by_its_pass_alone() {
    let Some(mut device) = vulkan_device() else {
        return;
    };
    let mut scene = Scene::default();
    let window = scene.add_window("t", (32, 32));
    let layer = scene.add_layer(window, None);
    {
        let state = scene.layer_mut(layer).expect("layer");
        state.rect = Rect::new(0, 0, 32, 32);
        state.visible = true;
    }
    let first = scene.add_bitmap(16, 16, vec![0u8; 16 * 16 * 4]);
    let second = scene.add_bitmap(16, 16, vec![0u8; 16 * 16 * 4]);
    {
        let state = scene.layer_mut(layer).expect("layer");
        state.bitmap = Some(first);
    }
    let child_bitmap = scene.add_bitmap(4, 4, vec![0u8; 4 * 4 * 4]);
    let child = scene.add_layer(window, Some(layer));
    {
        let state = scene.layer_mut(child).expect("child");
        state.rect = Rect::new(4, 4, 4, 4);
        state.visible = true;
        state.bitmap = Some(child_bitmap);
    }
    // The layer caches its subtree: the transition replaces the image the
    // cache holds.
    let cache = scene.add_bitmap(32, 32, vec![0u8; 32 * 32 * 4]);
    {
        let state = scene.layer_mut(layer).expect("layer");
        state.cache_enabled_count = 1;
        state.cache_bitmap = Some(cache);
    }
    scene.set_effect(layer, Some(cross_fade_draw(first, second, (16, 16), false)));
    if let Some(pipeline) = cross_fade_pipeline(&mut device) {
        scene.effect_pipelines.insert(Program::CrossFade, pipeline);
    }

    let textures = textures_for(&scene, &mut device);
    let report = project_window_report(&scene, window, |id| textures.get(&id).cloned());

    let effects = report
        .scene
        .nodes
        .iter()
        .filter(|node| matches!(node.content, RenderContent::Effect(_)))
        .count();
    assert_eq!(effects, 1, "the transition is one blend");
    assert!(
        report
            .scene
            .nodes
            .iter()
            .all(|node| node.dst != RectF::new(0.0, 0.0, 32.0, 32.0)),
        "the layer's cached subtree is not drawn"
    );
    assert!(
        report
            .scene
            .nodes
            .iter()
            .any(|node| node.dst == RectF::new(4.0, 4.0, 4.0, 4.0)),
        "the child keeps drawing live"
    );
}

/// A transition that covers the layer's whole subtree holds the children:
/// they are reported like the children a cache covers
/// (`reference/cpp/core/visual/LayerIntf.cpp:7221-7226`).
#[test]
fn a_whole_subtree_transition_holds_its_children() {
    let Some(mut device) = vulkan_device() else {
        return;
    };
    let mut scene = Scene::default();
    let window = scene.add_window("t", (16, 16));
    let layer = scene.add_layer(window, None);
    {
        let state = scene.layer_mut(layer).expect("layer");
        state.rect = Rect::new(0, 0, 16, 16);
        state.visible = true;
    }
    let child_bitmap = scene.add_bitmap(4, 4, vec![0u8; 4 * 4 * 4]);
    let child = scene.add_layer(window, Some(layer));
    {
        let state = scene.layer_mut(child).expect("child");
        state.rect = Rect::new(2, 2, 4, 4);
        state.visible = true;
        state.bitmap = Some(child_bitmap);
    }
    let first = scene.add_bitmap(16, 16, vec![0u8; 16 * 16 * 4]);
    let second = scene.add_bitmap(16, 16, vec![0u8; 16 * 16 * 4]);
    scene.set_effect(layer, Some(cross_fade_draw(first, second, (16, 16), true)));
    if let Some(pipeline) = cross_fade_pipeline(&mut device) {
        scene.effect_pipelines.insert(Program::CrossFade, pipeline);
    }

    let textures = textures_for(&scene, &mut device);
    let report = project_window_report(&scene, window, |id| textures.get(&id).cloned());

    let effects = report
        .scene
        .nodes
        .iter()
        .filter(|node| matches!(node.content, RenderContent::Effect(_)))
        .count();
    assert_eq!(effects, 1, "the transition is one blend");
    assert!(
        report.drawn_layers.contains(&child),
        "the covered child is reported: {:?}",
        report.drawn_layers
    );
}

/// A transition that covers only the layer's own image follows the image
/// placement, so a scaled image scales the transition with it.
#[test]
fn a_transition_follows_the_image_placement() {
    let Some(mut device) = vulkan_device() else {
        return;
    };
    let mut scene = Scene::default();
    let window = scene.add_window("t", (64, 64));
    let layer = scene.add_layer(window, None);
    {
        let state = scene.layer_mut(layer).expect("layer");
        state.rect = Rect::new(10, 10, 8, 8);
        state.visible = true;
    }
    let first = scene.add_bitmap(4, 4, vec![0u8; 4 * 4 * 4]);
    let second = scene.add_bitmap(4, 4, vec![0u8; 4 * 4 * 4]);
    // The image is drawn at twice its bitmap size.
    {
        let state = scene.layer_mut(layer).expect("layer");
        state.image_width = 8;
        state.image_height = 8;
    }
    scene.set_effect(layer, Some(cross_fade_draw(first, second, (4, 4), false)));
    if let Some(pipeline) = cross_fade_pipeline(&mut device) {
        scene.effect_pipelines.insert(Program::CrossFade, pipeline);
    }

    let textures = textures_for(&scene, &mut device);
    let report = project_window_report(&scene, window, |id| textures.get(&id).cloned());

    let placed: Vec<RectF> = report
        .scene
        .nodes
        .iter()
        .filter(|node| matches!(node.content, RenderContent::Effect(_)))
        .map(|node| node.dst)
        .collect();
    assert_eq!(placed.len(), 1, "the transition is one blend");
    assert_eq!(
        placed[0],
        RectF::new(10.0, 10.0, 8.0, 8.0),
        "the transition follows the image placement"
    );
}

/// Clearing the transition returns the layer to its own image.
#[test]
fn clearing_the_transition_returns_the_main_image() {
    let Some(mut device) = vulkan_device() else {
        return;
    };
    let mut scene = Scene::default();
    let window = scene.add_window("t", (16, 16));
    let layer = scene.add_layer(window, None);
    {
        let state = scene.layer_mut(layer).expect("layer");
        state.rect = Rect::new(0, 0, 8, 8);
        state.visible = true;
    }
    let main = scene.add_bitmap(8, 8, vec![0u8; 8 * 8 * 4]);
    {
        let state = scene.layer_mut(layer).expect("layer");
        state.bitmap = Some(main);
    }
    let first = scene.add_bitmap(8, 8, vec![0u8; 8 * 8 * 4]);
    let second = scene.add_bitmap(8, 8, vec![0u8; 8 * 8 * 4]);
    scene.set_effect(layer, Some(cross_fade_draw(first, second, (8, 8), true)));
    if let Some(pipeline) = cross_fade_pipeline(&mut device) {
        scene.effect_pipelines.insert(Program::CrossFade, pipeline);
    }

    let textures = textures_for(&scene, &mut device);
    let report = project_window_report(&scene, window, |id| textures.get(&id).cloned());
    let effects = report
        .scene
        .nodes
        .iter()
        .filter(|node| matches!(node.content, RenderContent::Effect(_)))
        .count();
    assert_eq!(effects, 1, "the transition is one blend");

    // The transition ends: the layer draws its own image again.
    scene.set_effect(layer, None);
    let report = project_window_report(&scene, window, |id| textures.get(&id).cloned());
    let effects = report
        .scene
        .nodes
        .iter()
        .filter(|node| matches!(node.content, RenderContent::Effect(_)))
        .count();
    assert_eq!(effects, 0, "the transition is gone");
    let textures_drawn = report
        .scene
        .nodes
        .iter()
        .filter(|node| matches!(node.content, RenderContent::Texture(_)))
        .count();
    assert_eq!(textures_drawn, 1, "the main image is back");
}
