//! Integration tests for the `shrinkCopy` plugin: `Layer.shrinkCopy` and
//! `Layer.shrinkCopyFast`.
//!
//! The oracle is the plugin's own source — the `shrinkCopy` repository named
//! by `reference/krkr2/docs/KRKR_PLUGINS.md:45` — whose `main.cpp` carries
//! the two members and whose `manual.tjs` states their behaviour. Every test
//! below cites the line of that source that defines what it pins:
//!
//! * `main.cpp:79-97` — the entry: nine arguments, the check, the clip and
//!   the copy;
//! * `main.cpp:108-118` — the parameter check: a rectangle that does not
//!   shrink is refused;
//! * `main.cpp:122-180` — the clipping of the source and the destination
//!   rectangles;
//! * `main.cpp:187-226` — the copy: the weighted area average;
//! * `main.cpp:369` — the attachment of `shrinkCopy` to `Layer`;
//! * `main.cpp:375-384,392-404,405-434,485` — `shrinkCopyFast`: the resize,
//!   the box average and the attachment;
//! * `manual.tjs:10-30,32-44` — the manual: the arguments, the refusal of
//!   an enlargement, the ignored face and hold-alpha, the ignored alpha of
//!   the fast copy and the resize.

mod common;

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

/// The plugin linked, with a `src` and a `dst` layer of 4x4 pixels.
fn two_layers(env: &Env) {
    env.link_plugin("shrinkCopy.dll");
    env.run(
        "var w = new Window(); \
         var src = new Layer(w, null); \
         var dst = new Layer(w, null); \
         src.name = 'src'; \
         dst.name = 'dst'; \
         src.setImageSize(4, 4); \
         dst.setImageSize(4, 4);",
    );
}

/// The plugin's members appear with the first link, and the link resolves to
/// the built-in: an unported plugin registers nothing, so the members would
/// stay absent.
///
/// The members are attached to `Layer` (`main.cpp:369,485`), so they are
/// members of the class object.
#[test]
fn the_link_resolves_to_the_builtin() {
    let _lock = vm_lock();
    let env = Env::new();
    assert_eq!(
        env.eval("typeof Layer.shrinkCopy").expect("probe"),
        TjsValue::String("undefined".into()),
        "the members are not registered before the link"
    );
    env.link_plugin("shrinkCopy.dll");
    assert_eq!(
        env.eval("typeof Layer.shrinkCopy").expect("probe"),
        TjsValue::String("Object".into()),
        "the link registers the member"
    );
    assert_eq!(
        env.eval("typeof Layer.shrinkCopyFast").expect("probe"),
        TjsValue::String("Object".into()),
        "the link registers the member"
    );
    env.run(
        "var linked = Plugins.getList(); \
         var found = 0; \
         for (var i = 0; i < linked.length; i++) { \
             if (linked[i] === 'shrinkcopy.dll') { found = 1; } \
         }",
    );
    assert_eq!(
        env.eval_int("found"),
        1,
        "the linked module is the plugin's DLL name"
    );
}

/// The area average: every destination pixel is the average of the source
/// pixels it covers.
///
/// The source is 2x2 with four different colours and the copy asks for a
/// 1x1 destination, so every source pixel covers a quarter of the
/// destination pixel. The weights are equal and the divisor is the product
/// of the two spans' totals (`main.cpp:187-226`): the horizontal span is an
/// edge span (`main.cpp:261`, one destination column) with
/// `tc = ta = 256`, `bc = ba = 0`, `step = 1` and `total = 512`, and so is
/// the vertical one (`total = 512`), so the divisor is 512 x 512 = 262144.
/// The four source pixels reach the sum through the four regions of the
/// area — the corner, the two edges and the middle (`main.cpp:209-217`) —
/// each with the weight 256 x 256 = 65536. The red channel is therefore
/// (255 + 0 + 0 + 255) x 65536 / 262144 = 127.5, truncated to 127, and the
/// alpha is 255.
#[test]
fn the_area_average_averages_the_covered_pixels() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env);
    env.run(
        "src.fillRect(0, 0, 1, 1, 0xFFFF0000); \
         src.fillRect(1, 0, 1, 1, 0xFF00FF00); \
         src.fillRect(0, 1, 1, 1, 0xFF0000FF); \
         src.fillRect(1, 1, 1, 1, 0xFFFFFFFF);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.shrinkCopy(0, 0, 1, 1, src, 0, 0, 2, 2);");
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [127, 127, 127, 255],
        "the destination pixel is the mean of the four source pixels"
    );
}

/// The alpha is averaged with the colour, and the alpha of a source pixel
/// that is not covered does not count.
///
/// The source is 2x1: an opaque red pixel and a transparent red pixel. The
/// destination pixel covers both, so its alpha is 255/2 = 127 and its colour
/// is the opaque red. The arithmetic is the upstream's (`main.cpp:187-226`):
/// the horizontal span is an edge span with `total = 512` and the vertical
/// one — one source row — has `step = 0` and `total = 256`, so the divisor
/// is 131072. The two source pixels reach the sum through the corner and
/// the top edge (`main.cpp:209-210`), each with the weight 256 x 256 =
/// 65536: the red is (255 + 255) x 65536 / 131072 = 255 and the alpha is
/// (255 + 0) x 65536 / 131072 = 127.5, truncated to 127.
#[test]
fn the_area_average_averages_the_alpha() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env);
    env.run(
        "src.fillRect(0, 0, 1, 1, 0xFFFF0000); \
         src.fillRect(1, 0, 1, 1, 0x00FF0000);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.shrinkCopy(0, 0, 1, 1, src, 0, 0, 2, 1);");
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [255, 0, 0, 127],
        "the alpha is averaged with the colour"
    );
}

/// The source rectangle is clipped to the source image, and the part of the
/// destination that is cut away is not copied.
///
/// The source is 2x1 (a red and a green pixel) and the copy asks for the
/// source rectangle `(-1, 0, 2, 1)`: the source pixel at -1 does not exist,
/// so the rectangle is clipped to the one pixel that does (`main.cpp:132-140`:
/// `sw += sx`, `dw -= zx * (-sx)`, `sx = 0`, `dx += zx * (-sx)`) and the
/// destination shrinks with it. The green pixel is outside the clipped
/// rectangle, so the destination holds the red one alone: the horizontal
/// span is an edge span whose far end is clamped to the rectangle
/// (`main.cpp:284-301`), so its weight reaches the boundary pixel alone and
/// the green pixel — the one the rectangle no longer covers — is not read
/// (its weight is zero, `main.cpp:209-217`).
#[test]
fn the_source_rectangle_is_clipped_to_the_source_image() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env);
    env.run(
        "src.fillRect(0, 0, 1, 1, 0xFFFF0000); \
         src.fillRect(1, 0, 1, 1, 0xFF00FF00);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.shrinkCopy(0, 0, 1, 1, src, -1, 0, 2, 1);");
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [255, 0, 0, 255],
        "the copy reads the source rectangle that is inside the source image"
    );
}

/// The destination rectangle is clipped to the destination image.
///
/// The destination is 4x4 and the copy asks for the rectangle
/// `(3, 3, 2, 2)`, which reaches past the image's right and bottom edges
/// (`main.cpp:157-180`: `dex = diw - dtx`). Only the pixel at (3, 3) is
/// inside, so only that pixel is written and the rest of the destination
/// keeps its content.
#[test]
fn the_destination_rectangle_is_clipped_to_the_destination_image() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env);
    env.run("src.fillRect(0, 0, 4, 4, 0xFFFF0000);");
    let dst = layer_id(&env, "dst");
    // The untouched image is the reference's default image: transparent
    // white, filled by `setImageSize`.
    env.run("dst.shrinkCopy(3, 3, 2, 2, src, 0, 0, 4, 4);");
    assert_eq!(
        pixel(&env, dst, 3, 3),
        [255, 0, 0, 255],
        "the pixel inside the destination image is written"
    );
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [255, 255, 255, 0],
        "the pixels outside the destination rectangle keep their content"
    );
    assert_eq!(
        pixel(&env, dst, 2, 2),
        [255, 255, 255, 0],
        "the pixels the clipped rectangle reached are not written"
    );
}

/// A destination rectangle larger than the source rectangle is refused: the
/// copy only shrinks (`main.cpp:108-118`, `manual.tjs:10-30`).
#[test]
fn an_enlargement_raises() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env);
    let message = env.error("dst.shrinkCopy(0, 0, 4, 4, src, 0, 0, 2, 2);");
    assert!(
        message.contains("shrinkCopy"),
        "the error names the member: {message}"
    );
    // The same rule holds for the height.
    let message = env.error("dst.shrinkCopy(0, 0, 2, 4, src, 0, 0, 2, 2);");
    assert!(
        message.contains("shrinkCopy"),
        "the error names the member: {message}"
    );
}

/// The copy needs its nine arguments (`main.cpp:79-83`).
#[test]
fn the_area_average_needs_nine_arguments() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env);
    let message = env.error("dst.shrinkCopy(0, 0, 1, 1, src, 0, 0, 2);");
    assert!(
        message.contains("shrinkCopy"),
        "the error names the member: {message}"
    );
}

/// The fast copy resizes the destination to the size of the result and
/// averages whole boxes of source pixels (`main.cpp:392-434`,
/// `manual.tjs:32-44`).
///
/// The source is 2x2 and the copy halves it, so the destination becomes 1x1
/// (`main.cpp:398-404`: `nw = (siw + stepx - 1) / stepx`,
/// `nh = (sih + stepy - 1) / stepy`). The horizontal step comes first
/// (`shrinkLineX`, `main.cpp:467-470`): each output pixel is the sum of its
/// box divided by the box's size (`ShrinkLine`, `main.cpp:451-465`), so the
/// first row's red is (3 + 0) / 2 = 1 and the second row's is (1 + 0) / 2 = 0.
/// The vertical step comes second (`shrinkLineY`, `main.cpp:471-473`): the
/// red is (1 + 0) / 2 = 0. Every division truncates, so the result is 0 and
/// not the 0.25 a single division over the four pixels would give. The alpha
/// is not read (`ShrinkLine` writes 255), so the result is opaque although
/// the first source pixel is transparent.
///
/// The destination *layer* takes the size, not just the image behind it:
/// the upstream calls the layer's `setImageSize`
/// (`main.cpp:400-403`), and the engine's `InternalSetImageSize`
/// (`reference/krkr2/cpp/core/visual/LayerIntf.cpp:2623-2641`) shrinks the
/// layer's own rectangle with the image.
#[test]
fn the_fast_copy_resizes_the_destination_and_averages_boxes() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env);
    env.run(
        "src.setImageSize(2, 2); \
         src.fillRect(0, 0, 1, 1, 0x00030000); \
         src.fillRect(1, 0, 1, 1, 0xFF000000); \
         src.fillRect(0, 1, 1, 1, 0xFF010000); \
         src.fillRect(1, 1, 1, 1, 0xFF000000);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.shrinkCopyFast(src, 2, 2);");
    assert_eq!(
        env.eval_int("dst.imageWidth"),
        1,
        "the destination image takes the size of the result"
    );
    assert_eq!(env.eval_int("dst.imageHeight"), 1);
    assert_eq!(
        env.eval_int("dst.width"),
        1,
        "the destination layer takes the size of the result"
    );
    assert_eq!(env.eval_int("dst.height"), 1);
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [0, 0, 0, 255],
        "the boxes are averaged and the result is opaque"
    );
}

/// The fast copy keeps the pixels that do not fill a whole box: they become
/// one more pixel of the result (`main.cpp:405-434`, `manual.tjs:32-44`).
///
/// A 3x1 source halved becomes 2x1 (`main.cpp:398-404`:
/// `nw = (3 + 2 - 1) / 2 = 2`, `nh = (1 + 2 - 1) / 2 = 1`). The horizontal
/// step takes the whole boxes first (`shrinkLineX`, `main.cpp:467-470`:
/// `xdiv = siw / stepx = 1`, `xrem = siw - xdiv * stepx = 1`) and the
/// leftover pixels after them, so the first output is the average of the
/// first two source pixels — the red (255 + 0) / 2 = 127 — and the second
/// one is the third source pixel alone. The vertical step finds no whole
/// group (`div = sih / stepy = 0`) and averages the leftover row alone, so
/// the row's own values stand (`main.cpp:427-433`).
#[test]
fn the_fast_copy_keeps_the_remainder_of_a_row() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env);
    env.run(
        "src.setImageSize(3, 1); \
         src.fillRect(0, 0, 1, 1, 0xFFFF0000); \
         src.fillRect(1, 0, 1, 1, 0xFF000000); \
         src.fillRect(2, 0, 1, 1, 0xFF00FF00);",
    );
    let dst = layer_id(&env, "dst");
    env.run("dst.shrinkCopyFast(src, 2, 2);");
    assert_eq!(
        env.eval_int("dst.imageWidth"),
        2,
        "the row that does not fill a whole box becomes one more pixel"
    );
    assert_eq!(
        pixel(&env, dst, 0, 0),
        [127, 0, 0, 255],
        "the first pixel is the average of the first two source pixels"
    );
    assert_eq!(
        pixel(&env, dst, 1, 0),
        [0, 255, 0, 255],
        "the last pixel is the source pixel that was left over"
    );
}

/// The fast copy needs its two arguments (`main.cpp:375-381`).
#[test]
fn the_fast_copy_needs_two_arguments() {
    let _lock = vm_lock();
    let env = Env::new();
    two_layers(&env);
    let message = env.error("dst.shrinkCopyFast(src);");
    assert!(
        message.contains("shrinkCopyFast"),
        "the error names the member: {message}"
    );
}
