//! A source read as an opaque image (`RenderNode::opaque_source`).
//!
//! A copy that takes the destination's alpha reads its source as an opaque
//! image: the source's colour lands as it is, the source's own alpha is not
//! read, and the node's composed opacity is the coverage. The node fact is the
//! only way to say that, because no blend of the source with the destination
//! can express it.
//!
//! Both backends honour the fact and draw the same picture. The headless
//! backend composites with the crate's own software rasterizer, so the cases
//! below run without a device; the Vulkan case compares the two pictures when
//! a device exists and skips when it does not, exactly like the other Vulkan
//! cases in this crate.

use api::blend::{BlendMode, DstBlend};
use api::error::RenderError;
use api::geometry::{PixelRegion, RectF};
use render::{
    PipelineConfig, RenderContent, RenderNode, RenderPipeline, RenderScene, RenderState,
    RenderTarget, RenderTargetDesc, Texture, TextureDesc, begin_offscreen_render_state,
    begin_render_state, create_render_pipeline, create_render_target, create_texture,
    read_render_target, render_to_target, upload_texture,
};
use testkit::headless_window;

/// One draw: a single-texel source over a destination, with the node's blend,
/// composed opacity and flags.
#[derive(Clone, Copy)]
struct Draw {
    /// The source texel, including its own alpha.
    texel: [u8; 4],
    /// The blend the node composites with.
    blend: BlendMode,
    /// The node's composed opacity.
    opacity: f32,
    /// Whether the node reads its source as an opaque image.
    opaque_source: bool,
    /// Whether the node holds the destination's alpha.
    hold_alpha: bool,
    /// The colour the 1×1 target is cleared with, i.e. the destination.
    background: [u8; 4],
}

impl Draw {
    /// A copy of `texel` that replaces `background` at the full opacity.
    fn copy(texel: [u8; 4], background: [u8; 4]) -> Self {
        Self {
            texel,
            blend: BlendMode::Replace,
            opacity: 1.0,
            opaque_source: true,
            hold_alpha: false,
            background,
        }
    }
}

/// The normalised `[0, 1]` colour of a byte pixel.
fn normalised(pixel: [u8; 4]) -> [f32; 4] {
    [
        f32::from(pixel[0]) / 255.0,
        f32::from(pixel[1]) / 255.0,
        f32::from(pixel[2]) / 255.0,
        f32::from(pixel[3]) / 255.0,
    ]
}

/// The scene: one textured node covering a 1×1 target, over `background`.
fn one_texel_scene(texture: &Texture, draw: Draw) -> RenderScene {
    let mut scene = RenderScene::with_root_node();
    let root = scene.root;
    let child = scene.push(RenderNode {
        parent: Some(root),
        content: RenderContent::Texture(texture.clone()),
        dst: RectF::new(0.0, 0.0, 1.0, 1.0),
        src: PixelRegion::new(0, 0, 1, 1),
        opacity: draw.opacity,
        blend: draw.blend,
        opaque_source: draw.opaque_source,
        hold_alpha: draw.hold_alpha,
        ..RenderNode::default()
    });
    scene.node_mut(root).expect("the root node").children = vec![child];
    scene.background = Some(normalised(draw.background));
    scene
}

/// Composites `draw` into a 1×1 target on `pipeline` and returns the pixel the
/// target holds.
///
/// The target holds the pixels in the space the pipeline composites in, which
/// the engine's read-back brings into the straight form: a case whose
/// destination is transparent therefore observes the source's colour scaled by
/// its coverage.
fn composite_texel(
    pipeline: &RenderPipeline,
    state: &mut RenderState,
    draw: Draw,
) -> Result<[u8; 4], RenderError> {
    let target: RenderTarget = create_render_target(pipeline, RenderTargetDesc::new(1, 1))?;
    let texture = create_texture(pipeline, TextureDesc::new(1, 1))?;
    upload_texture(
        pipeline,
        state,
        &texture,
        PixelRegion::new(0, 0, 1, 1),
        &draw.texel,
    )?;
    let scene = one_texel_scene(&texture, draw);
    render_to_target(pipeline, state, &target, &scene)?;
    let mut pixel = [0u8; 4];
    read_render_target(
        pipeline,
        state,
        &target,
        PixelRegion::new(0, 0, 1, 1),
        &mut pixel,
    )?;
    Ok(pixel)
}

/// The pixel the headless backend's software rasterizer produces.
fn cpu_pixel(draw: Draw) -> [u8; 4] {
    let pipeline = RenderPipeline::null();
    let (_surface, mut state) = begin_render_state(&pipeline, &headless_window()).expect("begin");
    composite_texel(&pipeline, &mut state, draw).expect("the rasterizer draws the draw")
}

/// The largest difference between two pixels' channels.
fn channel_gap(left: [u8; 4], right: [u8; 4]) -> u32 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| u32::from(a.abs_diff(*b)))
        .max()
        .unwrap_or(0)
}

/// A copy that reads its source as an opaque image replaces the destination
/// with the source's colour and an opaque alpha.
///
/// Reference: the opaque copy forces the source's alpha to one
/// (`reference/krkr2/cpp/core/visual/gl/blend_functor_c.h:174-178`, the
/// `color_opaque_functor` returning `0xff000000 | s`), and the render method
/// built for it is `CopyOpaqueImage`
/// (`reference/krkr2/cpp/core/visual/ogl/RenderManager_ogl.cpp:3141-3147`,
/// which writes `s.a = 1.0`); `bmCopyOnAlpha` selects it at the full opacity
/// (`reference/krkr2/cpp/core/visual/RenderManager.cpp:2398-2399`).
#[test]
fn an_opaque_source_replaces_the_destination_with_an_opaque_pixel() {
    // The source's own alpha is 40 and the destination's is 64: neither
    // survives the copy.
    let draw = Draw::copy([200, 100, 50, 40], [0, 0, 255, 64]);
    assert_eq!(
        cpu_pixel(draw),
        [200, 100, 50, 255],
        "the copy replaces the destination and the pixel is opaque"
    );

    // The plain copy of the same source takes the source's alpha
    // (`Copy`, `reference/krkr2/cpp/core/visual/RenderManager.cpp:2396-2397`;
    // `const_copy_functor`,
    // `reference/krkr2/cpp/core/visual/gl/blend_functor_c.h:158-160`).
    let plain = Draw {
        opaque_source: false,
        ..draw
    };
    assert_eq!(
        cpu_pixel(plain),
        [200, 100, 50, 40],
        "without the fact the source's own alpha is read"
    );
}

/// Below the full opacity the copy blends by the opacity, and the source is
/// still read as an opaque image.
///
/// Reference: `bmCopyOnAlpha` below the full opacity selects
/// `ConstAlphaBlend_d`
/// (`reference/krkr2/cpp/core/visual/RenderManager.cpp:2398-2399`), whose
/// program mixes the destination's colour towards the source's by the opacity
/// and writes the composed alpha
/// (`reference/krkr2/cpp/core/visual/ogl/RenderManager_ogl.cpp:3184-3193`).
#[test]
fn an_opaque_source_below_the_full_opacity_blends_by_the_opacity() {
    // The source's alpha of 40 is not read: the opacity of 128 is the
    // coverage, so the transparent destination is mixed towards the source by
    // 128/255 and the pixel's alpha is that coverage.
    let draw = Draw {
        blend: BlendMode::SourceOver,
        opacity: 128.0 / 255.0,
        background: [0, 0, 0, 0],
        ..Draw::copy([200, 100, 50, 40], [0, 0, 0, 0])
    };
    assert_eq!(
        cpu_pixel(draw),
        [100, 50, 25, 128],
        "the colour is mixed by the opacity and the alpha is the coverage"
    );

    // Without the fact the source's alpha of 40 supplies the coverage, at
    // 128/255 · 40/255 = 20/255.
    let plain = Draw {
        opaque_source: false,
        ..draw
    };
    assert_eq!(
        cpu_pixel(plain),
        [16, 8, 4, 20],
        "without the fact the source's own alpha supplies the coverage"
    );
}

/// The held spelling of the copy keeps the destination's alpha.
///
/// Reference: the held copy is `CopyColor`
/// (`reference/krkr2/cpp/core/visual/RenderManager.cpp:2398-2399`), whose
/// kernel copies the source's colour and keeps the destination's alpha
/// (`reference/krkr2/cpp/core/visual/gl/blend_functor_c.h:162-166`, the
/// `color_copy_functor` returning `(d & 0xff000000) + (s & 0x00ffffff)`).
#[test]
fn a_held_opaque_source_copy_keeps_the_destination_alpha() {
    // The destination's alpha of 64 survives and its colour does not.
    let draw = Draw {
        hold_alpha: true,
        ..Draw::copy([200, 100, 50, 40], [0, 0, 255, 64])
    };
    assert_eq!(
        cpu_pixel(draw),
        [200, 100, 50, 64],
        "the destination's alpha is held and the source's colour replaces the destination's"
    );
}

/// A source read as an opaque image and a blend that reads the destination
/// cannot combine: those blends take the weight of the source's colour from
/// the source's alpha, which the fact does not read.
///
/// The combination is refused rather than drawn differently by the two
/// backends: a typed error is a tree this path cannot draw, and the caller
/// decides what to do with it.
#[test]
fn an_opaque_source_with_a_blend_that_reads_the_destination_is_refused() {
    let draw = Draw {
        blend: BlendMode::DestinationDependent(DstBlend::Darken),
        ..Draw::copy([200, 100, 50, 40], [255, 255, 255, 255])
    };

    let pipeline = RenderPipeline::null();
    let (_surface, mut state) = begin_render_state(&pipeline, &headless_window()).expect("begin");
    let error = composite_texel(&pipeline, &mut state, draw)
        .expect_err("the rasterizer cannot combine the two");
    assert!(
        matches!(error, RenderError::Unsupported { .. }),
        "the refusal is a typed error: {error}"
    );

    // The blend draws on its own; it is the fact it cannot combine with.
    let plain = Draw {
        opaque_source: false,
        ..draw
    };
    composite_texel(&pipeline, &mut state, plain).expect("the blend draws without the fact");

    // The device refuses the same combination (and skips without a device).
    let Ok(vulkan) = create_render_pipeline(PipelineConfig::vulkan()) else {
        return;
    };
    let Ok(mut vulkan_state) = begin_offscreen_render_state(&vulkan) else {
        return;
    };
    let error = composite_texel(&vulkan, &mut vulkan_state, draw)
        .expect_err("the device cannot combine the two");
    assert!(
        matches!(error, RenderError::Unsupported { .. }),
        "the refusal is a typed error: {error}"
    );
}

/// The device draws the picture the software rasterizer draws, for both faces
/// of the copy.
///
/// A machine without a Vulkan device skips the test, exactly like the other
/// Vulkan cases here.
#[test]
fn the_device_and_the_cpu_draw_the_same_picture_or_skip() {
    let Ok(pipeline) = create_render_pipeline(PipelineConfig::vulkan()) else {
        return;
    };
    let Ok(mut state) = begin_offscreen_render_state(&pipeline) else {
        return;
    };
    let draws = [
        // The replacement at the full opacity.
        Draw::copy([200, 100, 50, 40], [0, 0, 255, 64]),
        // The blend below the full opacity over a transparent destination.
        Draw {
            blend: BlendMode::SourceOver,
            opacity: 128.0 / 255.0,
            background: [0, 0, 0, 0],
            ..Draw::copy([200, 100, 50, 40], [0, 0, 0, 0])
        },
        // The held copy.
        Draw {
            hold_alpha: true,
            ..Draw::copy([200, 100, 50, 40], [0, 0, 255, 64])
        },
    ];
    for draw in draws {
        let gpu = composite_texel(&pipeline, &mut state, draw).expect("the device draws the draw");
        let cpu = cpu_pixel(draw);
        // The fragment stage's values are quantized to eight bits before the
        // fixed-function blend reads them, so a channel can land one level
        // away from the rasterizer's float arithmetic.
        let gap = channel_gap(gpu, cpu);
        assert!(
            gap <= 1,
            "the device and the CPU must draw the same picture: {gpu:?} vs {cpu:?}"
        );
    }
}
