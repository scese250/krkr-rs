//! The device's copies of the bitmaps follow the layers.
//!
//! The Vulkan backend takes one descriptor set per copy it holds, and the
//! pool it draws from holds a fixed number of sets. A run that shows an
//! asset and drops it again must give the set back: a copy that is never
//! given back fills the pool, and the run dies with "out of GPU memory"
//! after a while — a game that keeps its story graphics alive on the script's
//! side shows one asset after another and never releases them.
//!
//! The copies the device holds are the ones the picture needs: a copy is
//! made when a layer shows a bitmap and given back when no layer shows it
//! any more. The assets themselves stay where the script put them.

use api::error::RenderError;
use engine::engine::FrameTarget;
use engine::{Engine, EngineParts, GameFrame, Rect};
use render::{
    PipelineConfig, RenderPipeline, RenderTargetDesc, begin_offscreen_render_state,
    create_render_pipeline, create_render_target,
};

/// The number of assets the run shows and drops again.
///
/// The pool the Vulkan backend draws from holds 1024 sets, so the run goes
/// past the number of copies the device could hold at once.
const ASSETS: u32 = 1100;

/// The size of the frame the engine draws.
const SIZE: (u32, u32) = (16, 12);

/// The colour of an asset: opaque, so the picture says whether it was drawn.
const COLOUR: [u8; 4] = [200, 180, 160, 255];

/// The pixels of an asset.
fn pixels() -> Vec<u8> {
    COLOUR.repeat((SIZE.0 * SIZE.1) as usize)
}

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

/// An engine over the null backend, which every host has.
fn null_engine() -> Engine {
    let pipeline = create_render_pipeline(PipelineConfig::null()).expect("null pipeline");
    let parts = offscreen_parts(pipeline, SIZE).expect("the offscreen parts");
    Engine::new(parts).expect("engine")
}

/// An engine over the Vulkan backend, or `None` when the host has no device.
///
/// The pool is the Vulkan descriptor pool, so the comparison runs on the
/// Vulkan backend. A host without a device skips it: the null backend has no
/// pool, and the engine's own table is what the null-backend test reads.
fn vulkan_engine() -> Option<Engine> {
    let pipeline = create_render_pipeline(PipelineConfig::vulkan()).ok()?;
    let parts = offscreen_parts(pipeline, SIZE).ok()?;
    Some(Engine::new(parts).expect("engine"))
}

/// A scene with one layer that shows the bitmap it is given.
fn layer_of(engine: &mut Engine) -> u32 {
    let scene = engine.scene_mut();
    let window = scene.add_window("test", SIZE);
    let layer = scene.add_layer(window, None);
    {
        let state = scene.layer_mut(layer).expect("the layer");
        state.visible = true;
        state.rect = Rect::new(0, 0, SIZE.0, SIZE.1);
        state.image_width = SIZE.0;
        state.image_height = SIZE.1;
    }
    layer
}

/// The layer shows `bitmap`.
fn show(engine: &mut Engine, layer: u32, bitmap: u32) {
    let scene = engine.scene_mut();
    let layer = scene.layer_mut(layer).expect("the layer");
    layer.bitmap = Some(bitmap);
}

/// The layer shows nothing: the bitmap it showed is alive on the script's
/// side, and no layer holds it any more.
fn show_nothing(engine: &mut Engine, layer: u32) {
    let scene = engine.scene_mut();
    let layer = scene.layer_mut(layer).expect("the layer");
    layer.bitmap = None;
}

/// The colour the frame the engine drew holds at its first pixel.
fn drawn_colour(engine: &Engine) -> [u8; 4] {
    let pixels = engine
        .read_offscreen_frame()
        .expect("the read-back succeeds")
        .expect("the engine draws into a target");
    let mut colour = [0u8; 4];
    colour.copy_from_slice(&pixels[0..4]);
    colour
}

/// A run that shows an asset, drops it, and shows the next one.
///
/// The assets stay alive on the script's side: the run keeps every bitmap it
/// has shown, the way a game keeps its story graphics. The copies the device
/// holds follow the layers, so the device holds the copy of the asset the
/// layer shows and not one copy per asset the run has ever shown.
#[test]
fn the_copies_follow_the_layers() {
    let mut engine = null_engine();
    let layer = layer_of(&mut engine);
    let mut hooks = NoFrame;

    for index in 0..ASSETS {
        // The asset, shown on the layer.
        let bitmap = engine.scene_mut().add_bitmap(SIZE.0, SIZE.1, pixels());
        show(&mut engine, layer, bitmap);
        engine
            .frame(u64::from(index) * 16, &mut hooks)
            .expect("the frame that shows the asset runs");

        // The layer shows something else. The asset stays alive: the script
        // keeps it, the way a game keeps its story graphics.
        show_nothing(&mut engine, layer);
        engine
            .frame(u64::from(index) * 16 + 8, &mut hooks)
            .expect("the frame that drops the asset runs");
    }

    assert!(
        engine.textures().len() <= 1,
        "the device holds {} copies after {ASSETS} assets: the copies follow the layers",
        engine.textures().len()
    );
}

/// The asset the layer shows again is copied to the device again.
///
/// The copy was given back when no layer held the asset; a layer that shows
/// it again needs the copy, and the whole asset is uploaded into it.
#[test]
fn an_asset_shown_again_is_copied_again() {
    let mut engine = null_engine();
    let layer = layer_of(&mut engine);
    let mut hooks = NoFrame;

    let bitmap = engine.scene_mut().add_bitmap(SIZE.0, SIZE.1, pixels());
    show(&mut engine, layer, bitmap);
    engine.frame(0, &mut hooks).expect("the frame runs");
    assert_eq!(engine.textures().len(), 1, "the layer shows the asset");

    show_nothing(&mut engine, layer);
    engine.frame(16, &mut hooks).expect("the frame runs");
    assert!(
        engine.textures().is_empty(),
        "no layer holds the asset, so the device holds no copy of it"
    );

    // The layer shows the asset again: the copy is made again and the whole
    // asset is uploaded into it, so the layer is drawn with the asset's
    // pixels.
    show(&mut engine, layer, bitmap);
    engine.frame(32, &mut hooks).expect("the frame runs");
    assert_eq!(
        engine.textures().len(),
        1,
        "the layer shows the asset, so the device holds its copy again"
    );
    assert_eq!(
        drawn_colour(&engine),
        COLOUR,
        "the frame holds the asset's pixels"
    );
}

/// A bitmap no layer shows is never copied to the device.
///
/// The pixels of such a bitmap stay on the host: the script reads them
/// there, and the device has no use for them. A copy would take a
/// descriptor set the pool cannot spare.
#[test]
fn a_bitmap_nobody_shows_takes_no_copy() {
    let mut engine = null_engine();
    let _layer = layer_of(&mut engine);
    let mut hooks = NoFrame;

    for _ in 0..100 {
        engine.scene_mut().add_bitmap(SIZE.0, SIZE.1, pixels());
    }
    engine.frame(0, &mut hooks).expect("the frame runs");
    assert!(
        engine.textures().is_empty(),
        "the device holds {} copies of the bitmaps nobody shows",
        engine.textures().len()
    );
}

/// The Vulkan pool survives a run that shows and drops more assets than it
/// could hold copies of at once.
///
/// The pool holds 1024 sets and the run shows 1100 assets. Before the copies
/// followed the layers, every asset kept its set and the pool ran out: the
/// device reported "out of GPU memory", the asset was never uploaded and the
/// layer was no longer drawn.
#[test]
fn the_pool_survives_a_run_that_shows_and_drops_many_assets() {
    let Some(mut engine) = vulkan_engine() else {
        eprintln!("the host has no Vulkan device: the pool is not checked");
        return;
    };
    let layer = layer_of(&mut engine);
    let mut hooks = NoFrame;

    for index in 0..ASSETS {
        let bitmap = engine.scene_mut().add_bitmap(SIZE.0, SIZE.1, pixels());
        show(&mut engine, layer, bitmap);
        engine
            .frame(u64::from(index) * 16, &mut hooks)
            .expect("the frame that shows the asset runs");

        show_nothing(&mut engine, layer);
        engine
            .frame(u64::from(index) * 16 + 8, &mut hooks)
            .expect("the frame that drops the asset runs");
    }

    // The asset the layer shows is drawn: the pool had room for its copy
    // and the copy holds the asset's pixels.
    let bitmap = engine.scene_mut().add_bitmap(SIZE.0, SIZE.1, pixels());
    show(&mut engine, layer, bitmap);
    engine
        .frame(u64::from(ASSETS) * 16, &mut hooks)
        .expect("the frame that shows the asset runs");
    assert_eq!(
        drawn_colour(&engine),
        COLOUR,
        "the frame holds the asset's pixels: the pool had room for its copy"
    );

    assert!(
        engine.textures().len() <= 1,
        "the device holds {} copies after {ASSETS} assets",
        engine.textures().len()
    );
}
