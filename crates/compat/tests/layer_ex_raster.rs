//! Integration tests for the `layerExRaster` plugin: `Layer.copyRaster`.
//!
//! The oracle is the clean-room specification in
//! `docs/code/compat/upstream/layerExRaster.md`, derived from the upstream
//! `layerExRaster` module (`main.cpp`, `manual.tjs`), its shared base class
//! (`layerExImage/layerExBase.hpp`) and `ncbind` (`ncbind.hpp`). Every test
//! below cites the upstream lines that define what it pins:
//!
//! * `layerExRaster/main.cpp:26` — the five-argument signature and `void`
//!   return type;
//! * `layerExRaster/main.cpp:32-42` — source image reads;
//! * `layerExRaster/main.cpp:44-46`, `layerExRaster/manual.tjs:12` — the
//!   size-mismatch guard (silent no-op, no redraw);
//! * `layerExRaster/main.cpp:49-61` — wave angular step, integer `H / 2`,
//!   initial phase and clip-top offset;
//! * `layerExRaster/main.cpp:65-83` — per-row sine displacement, copy ranges
//!   and untouched edge pixels;
//! * `layerExRaster/main.cpp:84`, `layerExImage/layerExBase.hpp:99-103` —
//!   redraw request whenever the size guard passes;
//! * `layerExRaster/main.cpp:90-101`, `layerExImage/layerExBase.hpp:76-89`,
//!   `:108-117` — destination clip rectangle refresh;
//! * `layerExRaster/main.cpp:109-111`, `layerExRaster/manual.tjs:18` —
//!   attachment of `copyRaster` to `Layer`;
//! * `ncbind/ncbind.hpp:1281` — fixed arity of five (fewer raises, extra
//!   ignored).

mod common;

use api::geometry::PixelRegion;
use common::{Env, vm_lock};
use tjs::TjsValue;

/// The scene id of the layer the script named `name`.
fn layer_id(env: &Env, name: &str) -> u32 {
    env.scene
        .layers
        .iter()
        .find(|layer| layer.name == name)
        .unwrap_or_else(|| panic!("no scene layer named {name:?}"))
        .id
}

/// The id of the main image of the layer the script named `name`.
fn image_id(env: &Env, name: &str) -> u32 {
    env.scene
        .layers
        .iter()
        .find(|layer| layer.name == name)
        .and_then(|layer| layer.bitmap)
        .unwrap_or_else(|| panic!("the layer {name:?} has no main image"))
}

/// The straight-alpha RGBA pixel at `(x, y)` of a layer's main image.
fn pixel(env: &Env, layer: u32, x: u32, y: u32) -> [u8; 4] {
    let bitmap = env
        .scene
        .layer(layer)
        .and_then(|layer| layer.bitmap)
        .expect("the layer has a main image");
    let bitmap = env.scene.bitmap(bitmap).expect("the bitmap exists");
    let offset = bitmap.pixel_offset(x, y).expect("the pixel is in range");
    [
        bitmap.rgba[offset],
        bitmap.rgba[offset + 1],
        bitmap.rgba[offset + 2],
        bitmap.rgba[offset + 3],
    ]
}

/// Fill a layer's main image with a unique `[x, y, 17, 255]` tag per pixel.
fn paint_coordinate_pattern(env: &mut Env, name: &str) {
    let id = image_id(env, name);
    let bitmap = env.scene.bitmap_mut(id).expect("the bitmap exists");
    let width = bitmap.width;
    let height = bitmap.height;
    for y in 0..height {
        for x in 0..width {
            let offset = bitmap.pixel_offset(x, y).expect("pixel in bounds");
            bitmap.rgba[offset] = (x & 0xFF) as u8;
            bitmap.rgba[offset + 1] = (y & 0xFF) as u8;
            bitmap.rgba[offset + 2] = 17;
            bitmap.rgba[offset + 3] = 255;
        }
    }
}

/// Fill a layer's main image with a constant RGBA sentinel pixel.
fn fill_sentinel(env: &mut Env, name: &str, rgba: [u8; 4]) {
    let id = image_id(env, name);
    let bitmap = env.scene.bitmap_mut(id).expect("the bitmap exists");
    for chunk in bitmap.rgba.chunks_exact_mut(4) {
        chunk.copy_from_slice(&rgba);
    }
}

/// Clear the destination layer's dirty bitmap state and repaint flags.
fn reset_dirty_and_paint(env: &mut Env, name: &str) {
    let dst = layer_id(env, name);
    let image = image_id(env, name);
    env.scene.set_bitmap_clean(image);
    let layer = env.scene.layer_mut(dst).expect("the layer exists");
    layer.pending_paint = false;
    layer.image_modified = false;
}

/// Link `layerExRaster.dll` and create `src` and `dst` layers of the given
/// image sizes.
fn two_layers(env: &Env, src: (i64, i64), dst: (i64, i64)) {
    env.link_plugin("layerExRaster.dll");
    env.run(&format!(
        "var w = new Window(); \
         var src = new Layer(w, null); \
         var dst = new Layer(w, null); \
         src.name = 'src'; \
         dst.name = 'dst'; \
         src.setImageSize({}, {}); \
         dst.setImageSize({}, {});",
        src.0, src.1, dst.0, dst.1
    ));
}

/// The link resolves to the built-in and attaches `copyRaster` to `Layer`
/// (`layerExRaster/main.cpp:109-111`, `layerExRaster/manual.tjs:18`).
#[test]
fn the_link_resolves_to_the_builtin() {
    let _lock = vm_lock();
    let env = Env::new();
    assert_eq!(
        env.eval("typeof Layer.copyRaster").expect("probe"),
        TjsValue::String("undefined".into()),
        "the member is not registered before the link"
    );
    env.link_plugin("layerExRaster.dll");
    assert_eq!(
        env.eval("typeof Layer.copyRaster").expect("probe"),
        TjsValue::String("Object".into()),
        "the link registers the member on Layer"
    );
    env.run(
        "var linked = Plugins.getList(); \
         var found = 0; \
         for (var i = 0; i < linked.length; i++) { \
             if (linked[i] === 'layerexraster.dll') { found = 1; } \
         }",
    );
    assert_eq!(
        env.eval_int("found"),
        1,
        "the linked module is the plugin's DLL name"
    );
}

/// Fewer than five arguments raises; extra arguments beyond five are ignored
/// (`ncbind/ncbind.hpp:1281`, `layerExRaster/main.cpp:26`,
/// `layerExRaster/manual.tjs:18`).
#[test]
fn arity_requires_five_and_ignores_extra_arguments() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (8, 8), (8, 8));
    let message = env.error("dst.copyRaster(src, 4, 16, 1000);");
    assert!(
        message.contains("copyRaster"),
        "the arity error names the member: {message}"
    );
    env.run("dst.copyRaster(src, 0, 16, 1000, 0, 'ignored', 123);");
}

/// The member returns nothing (`void`) (`layerExRaster/main.cpp:26`).
#[test]
fn the_member_returns_void() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (8, 8), (8, 8));
    assert_eq!(
        env.eval("dst.copyRaster(src, 0, 16, 1000, 0)")
            .expect("call"),
        TjsValue::Void,
        "copyRaster returns void"
    );
}

/// The worked example from the specification: 64x32 image, `lines = 16`,
/// `cycle = 1000`, `time = 0`, for both `maxh = 8` and `maxh = -8`
/// (`layerExRaster/main.cpp:49-83`).
#[test]
fn worked_example_positive_and_negative_amplitude() {
    let _lock = vm_lock();
    let mut env = Env::new();
    two_layers(&env, (64, 32), (64, 32));
    paint_coordinate_pattern(&mut env, "src");
    let sentinel = [0xAA, 0x55, 0x33, 0xFF];
    fill_sentinel(&mut env, "dst", sentinel);
    let dst = layer_id(&env, "dst");

    // Positive amplitude: rows shift rightwards; left edge keeps sentinel.
    env.run("dst.copyRaster(src, 8, 16, 1000, 0);");
    for (row, expected_d) in [(0_u32, 0_u32), (1, 3), (2, 5), (3, 7), (4, 8), (8, 0)] {
        for x in 0..expected_d {
            assert_eq!(
                pixel(&env, dst, x, row),
                sentinel,
                "row {row} col {x} (left of shift {expected_d}) keeps sentinel"
            );
        }
        for x in expected_d..64 {
            let src_x = x - expected_d;
            assert_eq!(
                pixel(&env, dst, x, row),
                [src_x as u8, row as u8, 17, 255],
                "row {row} col {x} copies source col {src_x}"
            );
        }
    }

    // Negative amplitude: rows shift leftwards; right edge keeps sentinel.
    fill_sentinel(&mut env, "dst", sentinel);
    env.run("dst.copyRaster(src, -8, 16, 1000, 0);");
    for (row, expected_shift) in [(0_u32, 0_u32), (1, 3), (2, 5), (3, 7), (4, 8), (8, 0)] {
        let written = 64 - expected_shift;
        for x in 0..written {
            let src_x = x + expected_shift;
            assert_eq!(
                pixel(&env, dst, x, row),
                [src_x as u8, row as u8, 17, 255],
                "row {row} col {x} copies source col {src_x}"
            );
        }
        for x in written..64 {
            assert_eq!(
                pixel(&env, dst, x, row),
                sentinel,
                "row {row} col {x} (right of shift -{expected_shift}) keeps sentinel"
            );
        }
    }
}

/// `phase0` uses integer division `H / 2` and advances with `time / cycle`
/// (`layerExRaster/main.cpp:49-59`).
#[test]
fn time_shifts_phase_using_integer_half_height() {
    let _lock = vm_lock();
    let mut env = Env::new();
    // Odd height H = 17 => half_height = 17 / 2 = 8 (integer division).
    // With lines = 16, cycle = 1000, time = 250:
    //   angular_step = 2*pi / 16
    //   phase0 = ((-angular_step * 250) / 1000) * 8 = -2 * angular_step = -pi/4
    //   row 0: sin(-pi/4) * 10 = -7.071... -> d = -7
    //   row 2: sin(0) * 10 = 0 -> d = 0
    //   row 6: sin(pi/2) * 10 = 10 -> d = 10
    two_layers(&env, (32, 17), (32, 17));
    paint_coordinate_pattern(&mut env, "src");
    let sentinel = [0x12, 0x34, 0x56, 0xFF];
    fill_sentinel(&mut env, "dst", sentinel);
    let dst = layer_id(&env, "dst");

    env.run("dst.copyRaster(src, 10, 16, 1000, 250);");

    // Row 0: d = -7 -> dst[0..25] gets src[7..32], dst[25..32] keeps sentinel.
    assert_eq!(pixel(&env, dst, 0, 0), [7, 0, 17, 255]);
    assert_eq!(pixel(&env, dst, 24, 0), [31, 0, 17, 255]);
    assert_eq!(pixel(&env, dst, 25, 0), sentinel);

    // Row 2: d = 0 -> verbatim.
    assert_eq!(pixel(&env, dst, 0, 2), [0, 2, 17, 255]);
    assert_eq!(pixel(&env, dst, 31, 2), [31, 2, 17, 255]);

    // Row 6: d = +10 -> dst[0..10] keeps sentinel, dst[10..32] gets src[0..22].
    assert_eq!(pixel(&env, dst, 9, 6), sentinel);
    assert_eq!(pixel(&env, dst, 10, 6), [0, 6, 17, 255]);
    assert_eq!(pixel(&env, dst, 31, 6), [21, 6, 17, 255]);
}

/// `maxh = 0` copies every clip row verbatim, and 32-bit pixels (including
/// transparent alpha) overwrite destination pixels without blending
/// (`layerExRaster/main.cpp:69-80`).
#[test]
fn zero_amplitude_copies_verbatim_including_alpha() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (4, 4), (4, 4));
    env.run(
        "dst.fillRect(0, 0, 4, 4, 0xFF445566); \
         src.fillRect(0, 0, 4, 4, 0x00112233);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.copyRaster(src, 0, 16, 1000, 500);");
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(
                pixel(&env, dst, x, y),
                [0x11, 0x22, 0x33, 0x00],
                "transparent source pixel ({x}, {y}) overwrites opaque destination verbatim"
            );
        }
    }
}

/// When `|d| >= clipWidth`, nothing is copied for that row, and the redraw is
/// still requested (`layerExRaster/main.cpp:65-84`,
/// `layerExImage/layerExBase.hpp:99-103`).
#[test]
fn displacement_exceeding_clip_width_skips_row_and_requests_redraw() {
    let _lock = vm_lock();
    let mut env = Env::new();
    two_layers(&env, (8, 8), (8, 8));
    paint_coordinate_pattern(&mut env, "src");
    let sentinel = [0x99, 0x88, 0x77, 0xFF];
    fill_sentinel(&mut env, "dst", sentinel);
    reset_dirty_and_paint(&mut env, "dst");
    let dst = layer_id(&env, "dst");

    // lines = 4: row 0 -> sin(0)=0 (d=0), row 1 -> sin(pi/2)=1 (d=16 >= 8),
    // row 2 -> sin(pi)=0 (d=0), row 3 -> sin(3pi/2)=-1 (d=-16, |d| >= 8).
    env.run("dst.copyRaster(src, 16, 4, 1000, 0);");

    for x in 0..8 {
        assert_eq!(pixel(&env, dst, x, 0), [x as u8, 0, 17, 255]);
        assert_eq!(pixel(&env, dst, x, 1), sentinel);
        assert_eq!(pixel(&env, dst, x, 2), [x as u8, 2, 17, 255]);
        assert_eq!(pixel(&env, dst, x, 3), sentinel);
    }
    assert!(env.scene.layer(dst).expect("layer").pending_paint);
    assert!(env.scene.layer(dst).expect("layer").image_modified);
}

/// The destination's clip rectangle applies to both source and destination
/// (including `phase0 += angular_step * clipTop`), the source's own clip is
/// ignored, and the dirty rect matches the destination's clip rectangle
/// (`layerExImage/layerExBase.hpp:76-89`, `:108-117`,
/// `layerExRaster/main.cpp:59-84`).
#[test]
fn destination_clip_applies_to_both_and_source_clip_is_ignored() {
    let _lock = vm_lock();
    let mut env = Env::new();
    two_layers(&env, (32, 16), (32, 16));
    paint_coordinate_pattern(&mut env, "src");
    let sentinel = [0xCC, 0xBB, 0xAA, 0xFF];
    fill_sentinel(&mut env, "dst", sentinel);
    // Give src a tiny unrelated clip; give dst clip (8, 4, 16, 8).
    env.run(
        "src.setClip(0, 0, 2, 2); \
         dst.setClip(8, 4, 16, 8);",
    );
    reset_dirty_and_paint(&mut env, "dst");
    let dst = layer_id(&env, "dst");
    let image = image_id(&env, "dst");

    // lines = 16, time = 0, CT = 4 -> phase0 = 4 * (2pi/16) = pi/2 -> row 4 has d = 4.
    env.run("dst.copyRaster(src, 4, 16, 1000, 0);");

    // Pixels outside dst's clip are untouched.
    assert_eq!(pixel(&env, dst, 0, 0), sentinel);
    assert_eq!(pixel(&env, dst, 7, 4), sentinel);
    assert_eq!(pixel(&env, dst, 24, 4), sentinel);
    assert_eq!(pixel(&env, dst, 12, 3), sentinel);
    assert_eq!(pixel(&env, dst, 12, 12), sentinel);

    // Row 4 (clip row 0): d = 4 -> dst[8..12] keeps sentinel, dst[12..24] gets src[8..20].
    for x in 8..12 {
        assert_eq!(pixel(&env, dst, x, 4), sentinel);
    }
    for x in 12..24 {
        let src_x = x - 4;
        assert_eq!(pixel(&env, dst, x, 4), [src_x as u8, 4, 17, 255]);
    }

    let bitmap = env.scene.bitmap(image).expect("bitmap");
    assert!(bitmap.dirty);
    assert_eq!(bitmap.dirty_region, Some(PixelRegion::new(8, 4, 16, 8)));
    assert!(env.scene.layer(dst).expect("layer").pending_paint);
    assert!(env.scene.layer(dst).expect("layer").image_modified);
}

/// When source and destination image sizes differ in either dimension, the
/// call does nothing, writes no pixel, and requests no redraw
/// (`layerExRaster/main.cpp:44-46`, `layerExRaster/manual.tjs:12`).
#[test]
fn size_mismatch_is_silent_noop_without_redraw() {
    let _lock = vm_lock();
    let mut env = Env::new();
    two_layers(&env, (16, 16), (16, 8));
    paint_coordinate_pattern(&mut env, "src");
    let sentinel = [0x11, 0x22, 0x33, 0xFF];
    fill_sentinel(&mut env, "dst", sentinel);
    reset_dirty_and_paint(&mut env, "dst");
    let dst = layer_id(&env, "dst");
    let image = image_id(&env, "dst");

    env.run("dst.copyRaster(src, 4, 16, 1000, 0);");

    assert_eq!(pixel(&env, dst, 0, 0), sentinel);
    let bitmap = env.scene.bitmap(image).expect("bitmap");
    assert!(!bitmap.dirty, "size mismatch must not dirty the bitmap");
    assert!(
        !env.scene.layer(dst).expect("layer").pending_paint,
        "size mismatch must not request a repaint"
    );
    assert!(
        !env.scene.layer(dst).expect("layer").image_modified,
        "size mismatch must not mark image modified"
    );
}

/// An empty clip rectangle on same-size layers copies nothing but still
/// requests a redraw (`layerExRaster/main.cpp:65`, `:84`,
/// `layerExImage/layerExBase.hpp:99-103`).
#[test]
fn empty_clip_copies_nothing_but_requests_redraw() {
    let _lock = vm_lock();
    let mut env = Env::new();
    two_layers(&env, (8, 8), (8, 8));
    paint_coordinate_pattern(&mut env, "src");
    let sentinel = [0x44, 0x55, 0x66, 0xFF];
    fill_sentinel(&mut env, "dst", sentinel);
    env.run("dst.setClip(0, 0, 0, 0);");
    reset_dirty_and_paint(&mut env, "dst");
    let dst = layer_id(&env, "dst");

    env.run("dst.copyRaster(src, 2, 8, 1000, 0);");

    assert_eq!(pixel(&env, dst, 0, 0), sentinel);
    assert!(
        env.scene.layer(dst).expect("layer").pending_paint,
        "empty clip with matching sizes still requests a repaint"
    );
    assert!(
        env.scene.layer(dst).expect("layer").image_modified,
        "empty clip with matching sizes still marks image modified"
    );
}

/// Self-copy (`src === dst`) reads the unmodified source row without
/// clobbering pixels that have not been read yet
/// (`layerExRaster/main.cpp:65-83`).
#[test]
fn self_copy_reads_unmodified_source_row() {
    let _lock = vm_lock();
    let mut env = Env::new();
    two_layers(&env, (16, 16), (16, 16));
    paint_coordinate_pattern(&mut env, "src");
    let src = layer_id(&env, "src");

    // Row 4 with lines = 16, time = 0 has sin(pi/2) = 1 -> d = 4.
    env.run("src.copyRaster(src, 4, 16, 1000, 0);");

    // Columns 0..4 keep their original values; columns 4..16 hold original 0..12.
    for x in 0..4 {
        assert_eq!(pixel(&env, src, x, 4), [x as u8, 4, 17, 255]);
    }
    for x in 4..16 {
        assert_eq!(pixel(&env, src, x, 4), [(x - 4) as u8, 4, 17, 255]);
    }
}

/// Numeric arguments use TJS integer coercion, `maxh`/`lines`/`cycle` are
/// narrowed to 32-bit signed integers, and objects/octets raise
/// (`layerExRaster/main.cpp:26`).
#[test]
fn integer_coercion_and_32bit_narrowing() {
    let _lock = vm_lock();
    let mut env = Env::new();
    two_layers(&env, (16, 16), (16, 16));
    paint_coordinate_pattern(&mut env, "src");
    let sentinel = [0x01, 0x02, 0x03, 0xFF];
    fill_sentinel(&mut env, "dst", sentinel);
    let dst = layer_id(&env, "dst");

    // Real truncated toward zero (4.9 -> 4), string parsed ("16" -> 16),
    // 64-bit narrowed to low 32 bits (0x1000003E8 -> 1000), void -> 0.
    env.run("dst.copyRaster(src, 4.9, '16', 0x1000003E8, void);");
    // Row 4: sin(pi/2) * 4 = 4.
    assert_eq!(pixel(&env, dst, 3, 4), sentinel);
    assert_eq!(pixel(&env, dst, 4, 4), [0, 4, 17, 255]);

    // 64-bit maxh whose low 32 bits are negative (-4 = 0xFFFFFFFC).
    fill_sentinel(&mut env, "dst", sentinel);
    env.run("dst.copyRaster(src, 0x1FFFFFFFC, 16, 1000, 0);");
    // Row 4: sin(pi/2) * -4 = -4 -> dst[0..12] gets src[4..16], dst[12..16] sentinel.
    assert_eq!(pixel(&env, dst, 0, 4), [4, 4, 17, 255]);
    assert_eq!(pixel(&env, dst, 12, 4), sentinel);

    // Object and octet arguments raise.
    let message = env.error("dst.copyRaster(src, %[], 16, 1000, 0);");
    assert!(
        message.contains("maxh"),
        "error names the failing argument: {message}"
    );
    let message = env.error("dst.copyRaster(src, 4, 16, 1000, <% 01 %>);");
    assert!(
        message.contains("time"),
        "error names the failing argument: {message}"
    );
}

/// Non-object or non-`Layer` sources and non-positive `lines`/`cycle` raise
/// typed errors rather than crashing or producing undefined behaviour
/// (`layerExRaster/main.cpp:32-34`, `layerExRaster/main.cpp:49-56`).
#[test]
fn invalid_source_and_non_positive_period_raise() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (8, 8), (8, 8));

    let message = env.error("dst.copyRaster(0, 4, 16, 1000, 0);");
    assert!(
        message.contains("copyRaster"),
        "non-object source raises: {message}"
    );
    let message = env.error("dst.copyRaster(%[], 4, 16, 1000, 0);");
    assert!(
        message.contains("Layer"),
        "non-Layer object source raises: {message}"
    );
    let message = env.error("dst.copyRaster(src, 4, 0, 1000, 0);");
    assert!(message.contains("lines"), "zero lines raises: {message}");
    let message = env.error("dst.copyRaster(src, 4, -8, 1000, 0);");
    assert!(
        message.contains("lines"),
        "negative lines raises: {message}"
    );
    let message = env.error("dst.copyRaster(src, 4, 16, 0, 0);");
    assert!(message.contains("cycle"), "zero cycle raises: {message}");
    let message = env.error("dst.copyRaster(src, 4, 16, -100, 0);");
    assert!(
        message.contains("cycle"),
        "negative cycle raises: {message}"
    );
}
