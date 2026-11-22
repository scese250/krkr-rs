//! Integration tests for the `layerExAreaAverage` plugin: `Layer.stretchCopyAA`.
//!
//! The oracle is the plugin's own source — the `layerExAreaAverage` module
//! whose `main.cpp` carries the member and whose `manual.tjs` states its
//! signature. Every test below cites the line of that source that defines
//! what it pins:
//!
//! * `main.cpp:16-21` — the member's name, its arity check and the
//!   "fewer than nine arguments" error;
//! * `main.cpp:29-36` — the four destination members the member reads, and
//!   the write address;
//! * `main.cpp:40-43` — the four destination numbers and their integer
//!   coercion;
//! * `main.cpp:48-56` — the source object and the four source members, with
//!   the read address;
//! * `main.cpp:60-63` — the four source numbers and their integer coercion;
//! * `main.cpp:65-70` — the magnification refusal;
//! * `main.cpp:72-96` — the four clipping steps, their order and their
//!   arithmetic;
//! * `main.cpp:98-107` — the source origin, the two truncated scale steps
//!   and the copy loop;
//! * `main.cpp:110-121` — the covered region of one destination pixel, the
//!   source pixel range and the two clamps;
//! * `main.cpp:126-158` — the visited range's exclusive ends, the overlaps
//!   and the accumulators;
//! * `main.cpp:163-183` — the destination pixel that covers nothing, the
//!   destination alpha, the colour and the written word;
//! * `main.cpp:189-199` — the update of the destination layer;
//! * `main.cpp:203` — the member returns nothing;
//! * `main.cpp:207-210` — the attachment to `Layer`;
//! * `manual.tjs:8-26` — the documented nine-parameter signature.
//!
//! The engine's own checkout carries no build of this module, so every
//! citation above points into the module's own tree.

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

/// The plugin linked, with a `src` and a `dst` layer of the given image
/// sizes.
fn two_layers(env: &Env, src: (i64, i64), dst: (i64, i64)) {
    env.link_plugin("layerExAreaAverage.dll");
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

/// The plugin linked, with a `src` and a `dst` layer of 4x4 pixels.
fn two_square_layers(env: &Env) {
    two_layers(env, (4, 4), (4, 4));
}

/// The members appear with the first link, and the link resolves to the
/// built-in: a module the engine does not know registers nothing, so the
/// member would stay absent.
///
/// The member is attached to `Layer` (`main.cpp:207-210`), so it is a member
/// of the class object.
#[test]
fn the_link_resolves_to_the_builtin() {
    let _lock = vm_lock();
    let env = Env::new();
    assert_eq!(
        env.eval("typeof Layer.stretchCopyAA").expect("probe"),
        TjsValue::String("undefined".into()),
        "the member is not registered before the link"
    );
    env.link_plugin("layerExAreaAverage.dll");
    assert_eq!(
        env.eval("typeof Layer.stretchCopyAA").expect("probe"),
        TjsValue::String("Object".into()),
        "the link registers the member"
    );
    env.run(
        "var linked = Plugins.getList(); \
         var found = 0; \
         for (var i = 0; i < linked.length; i++) { \
             if (linked[i] === 'layerExAreaAverage.dll') { found = 1; } \
         }",
    );
    assert_eq!(
        env.eval_int("found"),
        1,
        "the linked module is the plugin's DLL name"
    );
}

/// The copy needs its nine arguments (`main.cpp:16-21`, `manual.tjs:8-26`).
#[test]
fn the_copy_needs_nine_arguments() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    let message = env.error("dst.stretchCopyAA(0, 0, 1, 1, src, 0, 0, 2);");
    assert!(
        message.contains("stretchCopyAA"),
        "the error names the member: {message}"
    );
}

/// The copy only shrinks: a destination rectangle wider or taller than the
/// source rectangle is refused (`main.cpp:65-70`).
///
/// The check compares the sizes as the caller passed them and runs before
/// the clipping, so a destination that is only clipped down to the source's
/// size is refused as well.
#[test]
fn the_magnification_is_refused() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    let message = env.error("dst.stretchCopyAA(0, 0, 4, 4, src, 0, 0, 2, 2);");
    assert!(
        message.contains("stretchCopyAA"),
        "the error names the member: {message}"
    );
    // The same rule holds for the height.
    let message = env.error("dst.stretchCopyAA(0, 0, 2, 4, src, 0, 0, 2, 2);");
    assert!(
        message.contains("stretchCopyAA"),
        "the error names the member: {message}"
    );
    // A rectangle of the same size is a 1:1 copy and is allowed.
    env.run("dst.stretchCopyAA(0, 0, 2, 2, src, 0, 0, 2, 2);");
}

/// A copy of the same size leaves the destination's last column and last row
/// untouched (`main.cpp:116-121`, `main.cpp:136`, `main.cpp:163`).
///
/// The end of the visited source range is the column that holds the covered
/// region's right edge, and the range stops before it; the end is pulled
/// back to the source image's last column, so the source image's last column
/// and last row are never sampled and the destination pixels that cover
/// them are left as they were.
#[test]
fn a_copy_of_the_same_size_leaves_the_last_column_and_row_untouched() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    env.run("src.fillRect(0, 0, 4, 4, 0xFFFF0000);");
    let dst = layer_id(&env, "dst");
    env.run("dst.stretchCopyAA(0, 0, 4, 4, src, 0, 0, 4, 4);");
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(
                pixel(&env, dst, x, y),
                [255, 0, 0, 255],
                "the pixel ({x}, {y}) is the source's red"
            );
        }
    }
    // The untouched image is the reference's default image: transparent
    // white, filled by `setImageSize`.
    for (x, y) in [(3, 0), (0, 3), (3, 3)] {
        assert_eq!(
            pixel(&env, dst, x, y),
            [255, 255, 255, 0],
            "the pixel ({x}, {y}) covers no source pixel and keeps its content"
        );
    }
}

/// The source image's last column is never sampled (`main.cpp:116-121`,
/// `main.cpp:136`).
///
/// The source is 4x4 with three red columns and a green one, and the whole
/// image is copied into a 2x2 destination. The covered region of the
/// destination's right-hand column reaches the source image's right edge,
/// but the last column is never visited, so the green never mixes into the
/// result.
#[test]
fn the_source_images_last_column_is_never_sampled() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (4, 4), (2, 2));
    env.run(
        "src.fillRect(0, 0, 3, 4, 0xFFFF0000); \
         src.fillRect(3, 0, 1, 4, 0xFF00FF00);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.stretchCopyAA(0, 0, 2, 2, src, 0, 0, 4, 4);");
    for y in 0..2 {
        for x in 0..2 {
            assert_eq!(
                pixel(&env, dst, x, y),
                [255, 0, 0, 255],
                "the pixel ({x}, {y}) is red; the green column is never sampled"
            );
        }
    }
}

/// The colour is weighted by the area and the alpha, and the alpha of the
/// destination is the area-weighted mean of the source alphas
/// (`main.cpp:146-158`, `main.cpp:166`, `main.cpp:179-181`).
///
/// The source is 4x2: an opaque red pixel at the origin and a blue pixel
/// with the alpha 128 next to it. The source rectangle covers the 2x2 block
/// at the origin, but the source image's last row is never sampled, so only
/// the two pixels of row 0 contribute. The alpha is (255 + 128) / 2 = 191,
/// truncated, and the colour weights are (4096 * 255) >> 8 = 4080 and
/// (4096 * 128) >> 8 = 2048, so the red is 255 * 4080 / 6128 = 169 and the
/// blue is 255 * 2048 / 6128 = 85.
#[test]
fn the_colour_is_weighted_by_the_area_and_the_alpha() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (4, 2), (1, 1));
    env.run(
        "src.fillRect(0, 0, 1, 1, 0xFFFF0000); \
         src.fillRect(1, 0, 1, 1, 0x800000FF);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.stretchCopyAA(0, 0, 1, 1, src, 0, 0, 2, 2);");
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [169, 0, 85, 191],
        "the destination pixel is the alpha-weighted average of the two source pixels"
    );
}

/// A fully transparent source colours the result with its own colours
/// (`main.cpp:153-158`, `main.cpp:167-176`).
///
/// The two source pixels are transparent, so no pixel weighs on the colour
/// and the transparent sums answer instead: the area-weighted mean of the
/// two colours, (10 + 40) / 2 = 25, (20 + 50) / 2 = 35 and
/// (30 + 60) / 2 = 45.
#[test]
fn a_transparent_source_falls_back_to_its_own_colours() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (4, 2), (1, 1));
    env.run(
        "src.fillRect(0, 0, 1, 1, 0x000A141E); \
         src.fillRect(1, 0, 1, 1, 0x0028323C);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.stretchCopyAA(0, 0, 1, 1, src, 0, 0, 2, 1);");
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [25, 35, 45, 0],
        "the transparent pixels' own colours answer the colour"
    );
}

/// The destination rectangle is clipped to the destination image, and the
/// source rectangle is scaled down by the same ratio (`main.cpp:73-78`).
///
/// The destination image is 2x2 and the destination rectangle is
/// `(1, 1, 2, 2)`, so its width becomes 1 and the source rectangle is
/// scaled from 4x4 to 2x2. The single written destination pixel is the one
/// at (1, 1), and it holds the average of the source's 2x2 block at the
/// origin — the red block, not the green the rest of the source holds.
#[test]
fn the_destination_rectangle_is_clipped_to_the_destination_image() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (4, 4), (2, 2));
    env.run(
        "src.fillRect(0, 0, 2, 2, 0xFFFF0000); \
         src.fillRect(2, 0, 2, 4, 0xFF00FF00); \
         src.fillRect(0, 2, 4, 2, 0xFF00FF00);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.stretchCopyAA(1, 1, 2, 2, src, 0, 0, 4, 4);");
    assert_eq!(
        pixel(&env, dst, 1, 1),
        [255, 0, 0, 255],
        "the written pixel holds the average of the source's 2x2 block at the origin"
    );
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [255, 255, 255, 0],
        "the pixels the clipped rectangle did not reach keep their content"
    );
}

/// The source rectangle is clipped to the source image, and the destination
/// rectangle is scaled down by the same ratio (`main.cpp:79-84`).
///
/// The source rectangle `(2, 0, 4, 4)` reaches past the source image's
/// right edge, so the source width becomes 2 and the destination width
/// becomes 2 as well. The covered region of the destination's second column
/// reaches the source image's right edge, where the last column is never
/// sampled, so that column is left as it was.
#[test]
fn the_source_rectangle_is_clipped_to_the_source_image() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    env.run(
        "src.fillRect(0, 0, 2, 4, 0xFFFF0000); \
         src.fillRect(2, 0, 2, 4, 0xFF00FF00);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.stretchCopyAA(0, 0, 4, 4, src, 2, 0, 4, 4);");
    for y in 0..4 {
        assert_eq!(
            pixel(&env, dst, 0, y),
            [0, 255, 0, 255],
            "the first column holds the source's green column"
        );
        assert_eq!(
            pixel(&env, dst, 1, y),
            [255, 255, 255, 0],
            "the second column covers no source pixel and keeps its content"
        );
    }
}

/// The covered region of a destination pixel is anchored at the source
/// rectangle's origin (`main.cpp:98-99`, `main.cpp:110-113`).
///
/// The source rectangle starts at the column 2, so the copy reads the
/// source's columns from there and not from the source image's origin.
#[test]
fn the_covered_region_is_anchored_at_the_source_rectangles_origin() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (4, 4), (2, 2));
    env.run(
        "src.fillRect(0, 0, 2, 4, 0xFFFF0000); \
         src.fillRect(2, 0, 2, 4, 0xFF00FF00);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.stretchCopyAA(0, 0, 2, 2, src, 2, 0, 2, 2);");
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [0, 255, 0, 255],
        "the copy reads the source at the source rectangle's origin"
    );
    assert_eq!(
        pixel(&env, dst, 1, 0),
        [255, 255, 255, 0],
        "the second column covers no source pixel and keeps its content"
    );
}

/// Every destination pixel of a 3x3 copy is written
/// (`main.cpp:100-107`, `main.cpp:110-121`).
///
/// The source is 4x4 and the source rectangle is 3x3, so the covered region
/// of the last destination pixel is inside the source image and the copy
/// writes all nine pixels. The same call with a 4x4 destination is a
/// magnification and is refused.
#[test]
fn every_destination_pixel_of_a_three_by_three_copy_is_written() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env, (4, 4), (3, 3));
    env.run("src.fillRect(0, 0, 3, 3, 0xFFFF0000);");
    let dst = layer_id(&env, "dst");
    env.run("dst.stretchCopyAA(0, 0, 3, 3, src, 0, 0, 3, 3);");
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(
                pixel(&env, dst, x, y),
                [255, 0, 0, 255],
                "the pixel ({x}, {y}) is the source's red"
            );
        }
    }
    // The same call with a 4x4 destination is a magnification: the check
    // compares the sizes, and 4 > 3.
    let message = env.error("dst.stretchCopyAA(0, 0, 4, 4, src, 0, 0, 3, 3);");
    assert!(
        message.contains("stretchCopyAA"),
        "the error names the member: {message}"
    );
}

/// The copy writes the destination's main image and asks the destination
/// for a repaint (`main.cpp:189-199`).
///
/// The written rectangle is the part the frame has to upload, and the
/// destination layer is marked for a repaint.
#[test]
fn the_copy_marks_the_written_rectangle_and_the_layer() {
    let _lock = vm_lock();
    let mut env = Env::new();
    two_square_layers(&env);
    env.run("src.fillRect(0, 0, 4, 4, 0xFFFF0000);");
    let dst = layer_id(&env, "dst");
    let image = image_id(&env, "dst");
    env.scene.set_bitmap_clean(image);
    env.run("dst.stretchCopyAA(0, 0, 2, 2, src, 0, 0, 4, 4);");
    let bitmap = env.scene.bitmap(image).expect("the image");
    assert!(bitmap.dirty, "the write marks the image dirty");
    assert_eq!(
        bitmap.dirty_region,
        Some(PixelRegion::new(0, 0, 2, 2)),
        "the upload carries the rectangle the copy wrote"
    );
    assert!(
        env.scene.layer(dst).expect("the layer").pending_paint,
        "the destination is marked for a repaint"
    );
    assert!(
        env.scene.layer(dst).expect("the layer").image_modified,
        "the destination's image is marked modified"
    );
}

/// The member returns nothing (`main.cpp:203`).
#[test]
fn the_member_returns_nothing() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    env.run("src.fillRect(0, 0, 4, 4, 0xFFFF0000);");
    assert_eq!(
        env.eval("dst.stretchCopyAA(0, 0, 2, 2, src, 0, 0, 2, 2)")
            .expect("probe"),
        TjsValue::Void,
        "the copy returns nothing"
    );
}

/// A layer without an image has no pixels to copy from or to
/// (`main.cpp:35-36`, `main.cpp:55-56`).
///
/// The module reads the image's address and writes through it; a layer
/// without an image has no address, so the copy refuses the call instead of
/// writing through nothing.
#[test]
fn a_layer_without_an_image_raises() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    env.run("dst.hasImage = false;");
    let message = env.error("dst.stretchCopyAA(0, 0, 2, 2, src, 0, 0, 2, 2);");
    assert!(
        message.contains("main image"),
        "the error names what is missing: {message}"
    );
    env.run("src.hasImage = false;");
    let message = env.error("dst.stretchCopyAA(0, 0, 2, 2, src, 0, 0, 2, 2);");
    assert!(
        message.contains("main image"),
        "the error names what is missing: {message}"
    );
}

/// A negative origin addresses memory outside the image
/// (`main.cpp:105-106`, `main.cpp:128-129`).
///
/// The module's own arithmetic leaves that undefined, so the copy refuses
/// the call instead of writing outside the buffer.
#[test]
fn a_negative_origin_raises() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    let message = env.error("dst.stretchCopyAA(-1, 0, 2, 2, src, 0, 0, 2, 2);");
    assert!(
        message.contains("negative"),
        "the error names the problem: {message}"
    );
    let message = env.error("dst.stretchCopyAA(0, -1, 2, 2, src, 0, 0, 2, 2);");
    assert!(
        message.contains("negative"),
        "the error names the problem: {message}"
    );
    let message = env.error("dst.stretchCopyAA(0, 0, 2, 2, src, -1, 0, 2, 2);");
    assert!(
        message.contains("negative"),
        "the error names the problem: {message}"
    );
    let message = env.error("dst.stretchCopyAA(0, 0, 2, 2, src, 0, -1, 2, 2);");
    assert!(
        message.contains("negative"),
        "the error names the problem: {message}"
    );
}

/// The source is read as an object (`main.cpp:48`).
///
/// A number is not an object, and an object that is not a layer has no
/// image to read.
#[test]
fn the_source_must_be_a_layer() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    let message = env.error("dst.stretchCopyAA(0, 0, 2, 2, 0, 0, 0, 2, 2);");
    assert!(
        message.contains("stretchCopyAA"),
        "the error names the member: {message}"
    );
    let message = env.error("dst.stretchCopyAA(0, 0, 2, 2, %[], 0, 0, 2, 2);");
    assert!(
        message.contains("Layer"),
        "the error names what is missing: {message}"
    );
}

/// The numbers are read with the VM's integer conversion
/// (`main.cpp:40-43`, `main.cpp:60-63`).
///
/// An object cannot be converted to an integer, so the call raises; a real
/// is truncated toward zero and a string is parsed.
#[test]
fn the_numbers_are_read_with_the_integer_conversion() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    let message = env.error("dst.stretchCopyAA({}, 0, 2, 2, src, 0, 0, 2, 2);");
    assert!(
        message.contains("dleft"),
        "the error names the argument: {message}"
    );
    let message = env.error("dst.stretchCopyAA(0, 0, 2, 2, src, 0, 0, %[], 2);");
    assert!(
        message.contains("sheight"),
        "the error names the argument: {message}"
    );
    // A real is truncated toward zero and a string is parsed, so both name
    // the same rectangle as the integers do.
    env.run("src.fillRect(0, 0, 4, 4, 0xFFFF0000);");
    let dst = layer_id(&env, "dst");
    env.run("dst.stretchCopyAA(0.9, 0.9, 1.9, 1.9, src, 0, 0, 2, 2);");
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [255, 0, 0, 255],
        "the real arguments are truncated toward zero"
    );
}

/// The source and the destination may be the same layer
/// (`main.cpp:103-107`).
#[test]
fn the_source_and_the_destination_may_be_the_same_layer() {
    let _lock = vm_lock();
    let env = Env::new();
    two_square_layers(&env);
    env.run("src.fillRect(0, 0, 4, 4, 0xFFFF0000);");
    let src = layer_id(&env, "src");
    env.run("src.stretchCopyAA(0, 0, 2, 2, src, 0, 0, 4, 4);");
    assert_eq!(
        pixel(&env, src, 0, 0),
        [255, 0, 0, 255],
        "the copy reads the source it writes into"
    );
}
