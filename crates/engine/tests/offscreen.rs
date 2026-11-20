//! The engine's offscreen frame path: the frame is drawn into a target and
//! read back, with no surface and no display.
//!
//! A host with no window has no surface to present to, so the engine draws
//! its frame into an offscreen target instead and the caller reads the pixels
//! back. The null backend composites the target with its CPU rasterizer, so
//! the path is covered on every machine; the Vulkan backend draws the same
//! tree into a device image and reads it back the same way.

use api::blend::BlendMode;
use api::error::RenderError;
use engine::engine::FrameTarget;
use engine::{Engine, EngineError, EngineParts, GameFrame, Rect};
use render::{
    PipelineConfig, RenderPipeline, RenderTargetDesc, begin_offscreen_render_state,
    create_render_pipeline, create_render_target,
};

/// The size of the frame the engine draws.
const SIZE: (u32, u32) = (32, 24);

/// The colour of the layer that covers the frame.
const COLOUR: [u8; 4] = [24, 28, 40, 255];

/// A frame hook that changes nothing.
struct NoFrame;

impl GameFrame for NoFrame {
    fn before_render(&mut self, _now_ms: u64, _dt_ms: f64) -> Result<(), String> {
        Ok(())
    }
}

/// The parts of an engine that draws into an offscreen target of `size`.
fn offscreen_parts(pipeline: RenderPipeline, size: (u32, u32)) -> Result<EngineParts, RenderError> {
    let render_state = begin_offscreen_render_state(&pipeline)?;
    let target = create_render_target(&pipeline, RenderTargetDesc::new(size.0, size.1))?;
    Ok(EngineParts {
        pipeline,
        target: FrameTarget::Offscreen(target),
        render_state,
        surface_size: size,
        headless: true,
        overlay: None,
    })
}

/// Builds an engine that draws into an offscreen target of `size`.
fn offscreen_engine(size: (u32, u32)) -> Engine {
    let pipeline = create_render_pipeline(PipelineConfig::null()).expect("null pipeline");
    let parts = offscreen_parts(pipeline, size).expect("the offscreen parts");
    Engine::new(parts).expect("engine")
}

/// Fills the scene with one opaque layer that covers the frame.
fn fill_scene(engine: &mut Engine) {
    let scene = engine.scene_mut();
    let window = scene.add_window("test", SIZE);
    let layer = scene.add_layer(window, None);
    let pixels = COLOUR.repeat((SIZE.0 * SIZE.1) as usize);
    let bitmap = scene.add_bitmap(SIZE.0, SIZE.1, pixels);
    if let Some(layer) = scene.layer_mut(layer) {
        layer.visible = true;
        layer.rect = Rect::new(0, 0, SIZE.0, SIZE.1);
        layer.bitmap = Some(bitmap);
        layer.image_width = SIZE.0;
        layer.image_height = SIZE.1;
        layer.blend = BlendMode::Replace;
    }
}

/// The frame is drawn into the target, and the target holds the picture.
#[test]
fn the_frame_is_drawn_into_the_offscreen_target() {
    let mut engine = offscreen_engine(SIZE);
    fill_scene(&mut engine);

    let mut hooks = NoFrame;
    let report = engine.frame(0, &mut hooks).expect("the frame runs");
    assert_eq!(report.frame_index, 1);
    assert!(report.presented, "the frame reached the target");

    let pixels = engine
        .read_offscreen_frame()
        .expect("the read-back succeeds")
        .expect("the engine draws into a target");
    assert_eq!(pixels.len(), (SIZE.0 * SIZE.1 * 4) as usize);
    assert_eq!(
        &pixels[0..4],
        &COLOUR[..],
        "the target holds the picture the frame drew"
    );
}

/// The target keeps the frame until the next frame changes it.
#[test]
fn the_target_keeps_the_frame_of_the_last_draw() {
    let mut engine = offscreen_engine(SIZE);
    fill_scene(&mut engine);

    let mut hooks = NoFrame;
    engine.frame(0, &mut hooks).expect("the first frame");
    let first = engine
        .read_offscreen_frame()
        .expect("the read-back succeeds")
        .expect("the engine draws into a target");

    // The second frame changes nothing, so it draws nothing and the target
    // keeps the picture the first frame left.
    let report = engine.frame(16, &mut hooks).expect("the second frame");
    assert!(
        !report.presented,
        "a frame with nothing to show does not draw"
    );
    let second = engine
        .read_offscreen_frame()
        .expect("the read-back succeeds")
        .expect("the engine draws into a target");
    assert_eq!(first, second, "the target keeps the frame it holds");
}

/// The target is the frame's destination, so it must be the size the frame
/// is fitted into: a target of another size would crop the picture.
#[test]
fn a_target_of_another_size_is_refused() {
    let pipeline = create_render_pipeline(PipelineConfig::null()).expect("null pipeline");
    let mut parts = offscreen_parts(pipeline, SIZE).expect("the offscreen parts");
    // A target of another size than the surface size.
    let target = create_render_target(
        &parts.pipeline,
        RenderTargetDesc::new(SIZE.0 / 2, SIZE.1 / 2),
    )
    .expect("target");
    parts.target = FrameTarget::Offscreen(target);

    let error = match Engine::new(parts) {
        Ok(_) => panic!("a target of another size is refused"),
        Err(error) => error,
    };
    assert!(
        matches!(error, EngineError::Render(_)),
        "unexpected error: {error}"
    );
}

/// The GPU path is reachable without a window.
///
/// A Vulkan state created offscreen owns a logical device and no surface, so
/// the frame is drawn into a device image and read back on a machine that has
/// a GPU but no display.
///
/// A machine without a Vulkan device skips the test, exactly like the other
/// Vulkan cases in the render backend's own tests.
#[test]
fn the_vulkan_offscreen_frame_is_read_back_or_skip() {
    let Ok(pipeline) = create_render_pipeline(PipelineConfig::vulkan()) else {
        return;
    };
    let Ok(parts) = offscreen_parts(pipeline, SIZE) else {
        return;
    };
    let mut engine = Engine::new(parts).expect("engine");
    fill_scene(&mut engine);

    let mut hooks = NoFrame;
    let report = engine.frame(0, &mut hooks).expect("the frame runs");
    assert!(report.presented, "the frame reached the target");

    let pixels = engine
        .read_offscreen_frame()
        .expect("the read-back succeeds")
        .expect("the engine draws into a target");
    assert_eq!(pixels.len(), (SIZE.0 * SIZE.1 * 4) as usize);
    // The GPU path samples the texture in the target's own colour space, so
    // the round trip is within a level or two of the layer's colour.
    for (channel, expected) in pixels[0..4].iter().zip(COLOUR) {
        assert!(
            channel.abs_diff(expected) <= 2,
            "the target holds the picture the frame drew: {pixels:?}"
        );
    }
}
