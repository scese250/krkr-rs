//! Behaviour tests for the `layerExBTOA` plugin: the eight members the
//! plugin adds to the `Layer` class.
//!
//! The plugin is a standalone project whose sources are the oracle for these
//! members; every assertion below cites the line of `Main.cpp` (the single
//! translation unit of the plugin) that pins it.
//!
//! * `Main.cpp:139-169` — `copyRightBlueToLeftAlpha`
//! * `Main.cpp:175-205` — `copyBottomBlueToTopAlpha`
//! * `Main.cpp:207-229` — `fillAlpha`
//! * `Main.cpp:231-313` — `copyAlphaToProvince`
//! * `Main.cpp:389-511` — `clipAlphaRect`
//! * `Main.cpp:513-562` — `overwrapRect`
//! * `Main.cpp:565-612` — `fillByProvince`
//! * `Main.cpp:614-693` — `fillToProvince`
//!
//! The tests drive the script surface and read the pixels the natives wrote
//! out of the layer's image and its province plane.

mod common;

use common::vm_lock;
use engine::Scene;

/// The straight-alpha RGBA pixel at `(x, y)` of the named layer's main image.
fn pixel(env: &common::Env, name: &str, x: u32, y: u32) -> [u8; 4] {
    let layer = env
        .scene
        .layers
        .iter()
        .find(|layer| layer.name == name)
        .unwrap_or_else(|| panic!("no layer named {name:?}"));
    let bitmap_id = layer.bitmap.expect("the layer has a main image");
    let bitmap = env.scene.bitmap(bitmap_id).expect("the bitmap exists");
    let offset = bitmap.pixel_offset(x, y).expect("the pixel is in range");
    [
        bitmap.rgba[offset],
        bitmap.rgba[offset + 1],
        bitmap.rgba[offset + 2],
        bitmap.rgba[offset + 3],
    ]
}

/// The province byte at `(x, y)` of the named layer.
fn province(env: &common::Env, name: &str, x: u32, y: u32) -> u8 {
    let layer = env
        .scene
        .layers
        .iter()
        .find(|layer| layer.name == name)
        .unwrap_or_else(|| panic!("no layer named {name:?}"));
    let plane = layer.province.as_deref().expect("the layer has a province plane");
    let width = layer.province_width as usize;
    plane[y as usize * width + x as usize]
}

/// The named layer's record.
fn layer(env: &common::Env, name: &str) -> &engine::LayerState {
    env.scene
        .layers
        .iter()
        .find(|layer| layer.name == name)
        .unwrap_or_else(|| panic!("no layer named {name:?}"))
}

/// A window with one layer per entry, each `width` x `height` pixels.
fn make_layers(env: &common::Env, layers: &[(&str, u32, u32)]) {
    let mut script = String::from("var w = new Window();");
    for (name, width, height) in layers {
        script.push_str(&format!(
            "var {name} = new Layer(w, null); {name}.name = '{name}'; \
             {name}.setImageSize({width}, {height});"
        ));
    }
    env.run(&script);
}

/// The plugin's members are published when the script links the plugin.
fn link(env: &common::Env) {
    env.link_plugin("layerExBTOA.dll");
}

// ---------------------------------------------------------------------------
// copyRightBlueToLeftAlpha
// ---------------------------------------------------------------------------

/// The blue byte of the image's right half becomes the alpha byte of its
/// left half, and nothing else changes (`Main.cpp:153-165`).
///
/// The refresh covers the left half only (`Main.cpp:167`).
#[test]
fn copy_right_blue_to_left_alpha_moves_the_blue_byte_into_the_alpha_byte() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 4, 4)]);
    env.run(
        "l.fillRect(0, 0, 2, 4, 0x80112233); \
         l.fillRect(2, 0, 2, 4, 0xFF445566);",
    );

    env.run("l.copyRightBlueToLeftAlpha();");

    // The left half's alpha is the right half's blue byte (Main.cpp:153-165).
    assert_eq!(
        pixel(&env, "l", 0, 0),
        [0x11, 0x22, 0x33, 0x66],
        "the left half's alpha byte must be the right half's blue byte"
    );
    assert_eq!(
        pixel(&env, "l", 1, 3),
        [0x11, 0x22, 0x33, 0x66],
        "every row of the left half takes the same alpha byte"
    );
    // The left half's own blue byte and the right half are untouched.
    assert_eq!(
        pixel(&env, "l", 0, 0)[2],
        0x33,
        "the left half's blue byte must not change"
    );
    assert_eq!(
        pixel(&env, "l", 2, 0),
        [0x44, 0x55, 0x66, 0xff],
        "the right half must not change"
    );
}

/// An odd width leaves the last column of the image alone, because the two
/// halves do not meet there (`Main.cpp:150`).
#[test]
fn copy_right_blue_to_left_alpha_leaves_the_last_column_of_an_odd_width_alone() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 5, 2)]);
    env.run(
        "l.fillRect(0, 0, 5, 2, 0x80112233); \
         l.fillRect(2, 0, 3, 2, 0xFF445566);",
    );

    env.run("l.copyRightBlueToLeftAlpha();");

    // The halves are two columns wide, so the columns 0 and 1 take the blue
    // byte of the columns 2 and 3.
    assert_eq!(
        pixel(&env, "l", 0, 0),
        [0x11, 0x22, 0x33, 0x66],
        "the first column takes the blue byte of the third one"
    );
    assert_eq!(
        pixel(&env, "l", 1, 0),
        [0x11, 0x22, 0x33, 0x66],
        "the second column takes the blue byte of the fourth one"
    );
    // The last column is in neither half.
    assert_eq!(
        pixel(&env, "l", 4, 0),
        [0x44, 0x55, 0x66, 0xff],
        "the last column of an odd width must not change"
    );
}

// ---------------------------------------------------------------------------
// copyBottomBlueToTopAlpha
// ---------------------------------------------------------------------------

/// The blue byte of the image's bottom half becomes the alpha byte of its
/// top half, and the whole width takes part (`Main.cpp:189-201`).
#[test]
fn copy_bottom_blue_to_top_alpha_moves_the_blue_byte_into_the_alpha_byte() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 4)]);
    env.run(
        "l.fillRect(0, 0, 2, 2, 0x80112233); \
         l.fillRect(0, 2, 2, 2, 0xFF445566);",
    );

    env.run("l.copyBottomBlueToTopAlpha();");

    assert_eq!(
        pixel(&env, "l", 0, 0),
        [0x11, 0x22, 0x33, 0x66],
        "the top half's alpha byte must be the bottom half's blue byte"
    );
    assert_eq!(
        pixel(&env, "l", 1, 1),
        [0x11, 0x22, 0x33, 0x66],
        "the whole width takes part"
    );
    assert_eq!(
        pixel(&env, "l", 0, 2),
        [0x44, 0x55, 0x66, 0xff],
        "the bottom half must not change"
    );
}

// ---------------------------------------------------------------------------
// fillAlpha
// ---------------------------------------------------------------------------

/// The alpha byte of the clip rectangle becomes opaque and the colour bytes
/// keep their values (`Main.cpp:216-225`).
#[test]
fn fill_alpha_makes_the_clip_rectangle_opaque() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 4, 4)]);
    env.run(
        "l.fillRect(0, 0, 4, 4, 0x00112233); \
         l.setClip(1, 1, 2, 2);",
    );

    env.run("l.fillAlpha();");

    assert_eq!(
        pixel(&env, "l", 1, 1),
        [0x11, 0x22, 0x33, 0xff],
        "the clip rectangle's alpha byte must become opaque"
    );
    assert_eq!(
        pixel(&env, "l", 2, 2),
        [0x11, 0x22, 0x33, 0xff],
        "the whole clip rectangle is filled"
    );
    assert_eq!(
        pixel(&env, "l", 0, 0),
        [0x11, 0x22, 0x33, 0x00],
        "a pixel outside the clip rectangle must not change"
    );
}

// ---------------------------------------------------------------------------
// copyAlphaToProvince
// ---------------------------------------------------------------------------

/// Without a threshold the alpha byte is copied into the province plane as
/// it is (`Main.cpp:291-292`).
#[test]
fn copy_alpha_to_province_copies_the_alpha_byte_verbatim() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run(
        "l.fillRect(0, 0, 1, 1, 0x80112233); \
         l.fillRect(1, 0, 1, 1, 0x40112233);",
    );

    env.run("l.copyAlphaToProvince();");

    assert_eq!(
        province(&env, "l", 0, 0),
        0x80,
        "the province byte must be the alpha byte"
    );
    assert_eq!(
        province(&env, "l", 1, 0),
        0x40,
        "the province byte must be the alpha byte"
    );
}

/// With a threshold the pixels at or above it take the matched byte and the
/// pixels below it take the other one (`Main.cpp:294-296`).
#[test]
fn copy_alpha_to_province_thresholds_the_alpha_byte() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run(
        "l.fillRect(0, 0, 1, 1, 0x80112233); \
         l.fillRect(1, 0, 1, 1, 0x40112233);",
    );

    env.run("l.copyAlphaToProvince(0x80, 7, 9);");

    assert_eq!(
        province(&env, "l", 0, 0),
        7,
        "a pixel at the threshold takes the matched byte"
    );
    assert_eq!(
        province(&env, "l", 1, 0),
        9,
        "a pixel below the threshold takes the other byte"
    );
}

/// A matched byte outside the byte range is not written, so the pixels below
/// the threshold keep the province value they had (`Main.cpp:300-302`).
#[test]
fn copy_alpha_to_province_keeps_the_province_value_of_the_pixels_it_does_not_write() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run(
        "l.fillRect(0, 0, 1, 1, 0x80112233); \
         l.fillRect(1, 0, 1, 1, 0x40112233); \
         l.copyAlphaToProvince();",
    );

    // The other byte is outside the byte range, so only the matched side is
    // written.
    env.run("l.copyAlphaToProvince(0x80, 7, 300);");

    assert_eq!(
        province(&env, "l", 0, 0),
        7,
        "a pixel at the threshold takes the matched byte"
    );
    assert_eq!(
        province(&env, "l", 1, 0),
        0x40,
        "a pixel below the threshold keeps the province value it had"
    );
}

/// A threshold above the byte range names no pixel, so every pixel takes the
/// other byte (`Main.cpp:297-299`).
#[test]
fn copy_alpha_to_province_writes_the_other_byte_everywhere_above_the_byte_range() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run(
        "l.fillRect(0, 0, 1, 1, 0x80112233); \
         l.fillRect(1, 0, 1, 1, 0x40112233);",
    );

    env.run("l.copyAlphaToProvince(300, 7, 9);");

    assert_eq!(
        province(&env, "l", 0, 0),
        9,
        "every pixel takes the other byte"
    );
    assert_eq!(
        province(&env, "l", 1, 0),
        9,
        "every pixel takes the other byte"
    );
}

/// A threshold below zero names no threshold, so the alpha byte is copied as
/// it is (`Main.cpp:277`, `Main.cpp:282`, `Main.cpp:291-292`).
#[test]
fn copy_alpha_to_province_copies_verbatim_below_zero() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run(
        "l.fillRect(0, 0, 1, 1, 0x80112233); \
         l.fillRect(1, 0, 1, 1, 0x40112233);",
    );

    env.run("l.copyAlphaToProvince(-5);");

    assert_eq!(
        province(&env, "l", 0, 0),
        0x80,
        "a negative threshold copies the alpha byte"
    );
    assert_eq!(
        province(&env, "l", 1, 0),
        0x40,
        "a negative threshold copies the alpha byte"
    );
}

/// An argument that is passed as `void` counts as not passed, so the default
/// applies (`Main.cpp:238`, `Main.cpp:239-247`).
#[test]
fn copy_alpha_to_province_treats_a_void_argument_as_not_passed() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run(
        "l.fillRect(0, 0, 1, 1, 0x80112233); \
         l.fillRect(1, 0, 1, 1, 0x40112233);",
    );

    env.run("l.copyAlphaToProvince(void, void, void);");

    assert_eq!(
        province(&env, "l", 0, 0),
        0x80,
        "a void threshold copies the alpha byte"
    );
    assert_eq!(
        province(&env, "l", 1, 0),
        0x40,
        "a void threshold copies the alpha byte"
    );
}

// ---------------------------------------------------------------------------
// clipAlphaRect
// ---------------------------------------------------------------------------

/// The destination's alpha byte is multiplied by the source's alpha byte
/// over the rectangle, and the colour bytes keep their values
/// (`Main.cpp:448`, `Main.cpp:453`).
#[test]
fn clip_alpha_rect_multiplies_the_alpha_channel() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 4, 4)]);
    env.run(
        "dst.fillRect(0, 0, 4, 4, 0x80112233); \
         src.fillRect(0, 0, 4, 4, 0x40445566);",
    );

    env.run("dst.clipAlphaRect(0, 0, src, 0, 0, 2, 2);");

    // The scaling is `(c * a + ((c * a) >> 7)) >> 8` (Main.cpp:450-451):
    // 128 * 64 gives 32.
    assert_eq!(
        pixel(&env, "dst", 0, 0),
        [0x11, 0x22, 0x33, 32],
        "the destination's alpha byte must be scaled by the source's alpha byte"
    );
    assert_eq!(
        pixel(&env, "dst", 1, 1),
        [0x11, 0x22, 0x33, 32],
        "the whole rectangle is scaled"
    );
    assert_eq!(
        pixel(&env, "dst", 2, 0),
        [0x11, 0x22, 0x33, 0x80],
        "a pixel outside the rectangle must not change"
    );
}

/// A destination that stores an additive alpha has all four of its channels
/// scaled, the alpha included (`Main.cpp:466-468`, `Main.cpp:480-482`).
#[test]
fn clip_alpha_rect_scales_every_channel_of_an_add_alpha_destination() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 4, 4)]);
    env.run(
        "dst.type = 12; \
         dst.fillRect(0, 0, 4, 4, 0x80112233); \
         src.fillRect(0, 0, 4, 4, 0x40445566);",
    );

    env.run("dst.clipAlphaRect(0, 0, src, 0, 0, 2, 2);");

    // Every channel is scaled by the source's alpha byte (Main.cpp:315-328):
    // 17, 34 and 51 scaled by 64 give 4, 8 and 12, and 128 gives 32.
    assert_eq!(
        pixel(&env, "dst", 0, 0),
        [4, 8, 12, 32],
        "every channel of an additive alpha destination must be scaled"
    );
    assert_eq!(
        pixel(&env, "dst", 2, 0),
        [0x11, 0x22, 0x33, 0x80],
        "a pixel outside the rectangle must not change"
    );
}

/// The clear value writes the alpha byte over the part of the clip rectangle
/// the rectangle does not cover (`Main.cpp:440-445`, `Main.cpp:453`).
#[test]
fn clip_alpha_rect_clears_the_alpha_byte_outside_the_rectangle() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 4, 4)]);
    env.run(
        "dst.fillRect(0, 0, 4, 4, 0x80112233); \
         src.fillRect(0, 0, 4, 4, 0x40445566);",
    );

    env.run("dst.clipAlphaRect(0, 0, src, 0, 0, 2, 2, 0);");

    assert_eq!(
        pixel(&env, "dst", 0, 0),
        [0x11, 0x22, 0x33, 32],
        "the rectangle itself is scaled"
    );
    assert_eq!(
        pixel(&env, "dst", 3, 3),
        [0x11, 0x22, 0x33, 0x00],
        "the clear value writes the alpha byte outside the rectangle"
    );
}

/// A rectangle that reaches outside the source image is cut to it, and the
/// surviving source pixels keep the destination pixels the script asked for
/// (`Main.cpp:339-346`).
#[test]
fn clip_alpha_rect_cuts_the_rectangle_to_the_source_image() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 2, 2)]);
    env.run(
        "dst.fillRect(0, 0, 4, 4, 0x80112233); \
         src.fillRect(0, 0, 2, 2, 0x40445566);",
    );

    // The source rectangle starts left of the source image, so the first
    // column of the source is cut away and the destination moves with it.
    env.run("dst.clipAlphaRect(0, 0, src, -1, 0, 2, 2);");

    assert_eq!(
        pixel(&env, "dst", 0, 0),
        [0x11, 0x22, 0x33, 32],
        "the surviving source pixel keeps its destination pixel"
    );
    assert_eq!(
        pixel(&env, "dst", 1, 0),
        [0x11, 0x22, 0x33, 0x80],
        "the destination moves by the amount the source's edge was cut by"
    );
}

/// A rectangle that reaches outside the destination's clip rectangle is cut
/// to it (`Main.cpp:348-358`).
#[test]
fn clip_alpha_rect_cuts_the_rectangle_to_the_destination_clip() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 4, 4)]);
    env.run(
        "dst.fillRect(0, 0, 4, 4, 0x80112233); \
         src.fillRect(0, 0, 4, 4, 0x40445566); \
         dst.setClip(1, 1, 2, 2);",
    );

    env.run("dst.clipAlphaRect(0, 0, src, 0, 0, 4, 4);");

    assert_eq!(
        pixel(&env, "dst", 1, 1),
        [0x11, 0x22, 0x33, 32],
        "the part of the rectangle inside the clip rectangle is scaled"
    );
    assert_eq!(
        pixel(&env, "dst", 0, 0),
        [0x11, 0x22, 0x33, 0x80],
        "the part of the rectangle outside the clip rectangle is not written"
    );
}

/// A rectangle that reaches neither the source image nor the destination's
/// clip rectangle changes nothing, and a clear value still reaches the whole
/// clip rectangle (`Main.cpp:501-510`).
#[test]
fn clip_alpha_rect_writes_the_clear_value_over_the_whole_clip_rectangle() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 2, 2)]);
    env.run(
        "dst.fillRect(0, 0, 4, 4, 0x80112233); \
         src.fillRect(0, 0, 2, 2, 0x40445566);",
    );

    env.run("dst.clipAlphaRect(10, 10, src, 0, 0, 2, 2, 0x40);");

    assert_eq!(
        pixel(&env, "dst", 0, 0),
        [0x11, 0x22, 0x33, 0x40],
        "the clear value reaches every pixel of the clip rectangle"
    );
    assert_eq!(
        pixel(&env, "dst", 3, 3),
        [0x11, 0x22, 0x33, 0x40],
        "the clear value reaches every pixel of the clip rectangle"
    );
}

/// Fewer than seven arguments is a bad parameter count (`Main.cpp:402`).
#[test]
fn clip_alpha_rect_needs_seven_arguments() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 4, 4)]);

    let message = env.error("dst.clipAlphaRect(0, 0, src, 0, 0, 2)");
    assert!(
        message.contains("Invalid argument count"),
        "the error must be the bad parameter count: {message}"
    );
}

/// A width or a height that is not positive is a bad parameter
/// (`Main.cpp:416`).
#[test]
fn clip_alpha_rect_rejects_a_size_that_is_not_positive() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 4, 4)]);

    let message = env.error("dst.clipAlphaRect(0, 0, src, 0, 0, 0, 2)");
    assert!(
        message.contains("Invalid argument"),
        "the error must be the invalid parameter: {message}"
    );
}

/// A source that is not an object cannot become one, so the conversion fails
/// (`Main.cpp:406`).
#[test]
fn clip_alpha_rect_rejects_a_source_that_is_not_an_object() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4)]);

    let message = env.error("dst.clipAlphaRect(0, 0, 5, 0, 0, 2, 2)");
    assert!(
        message.contains("Cannot convert the variable type"),
        "the error must be the variant conversion: {message}"
    );
}

/// A source that is not a layer is rejected with the member's own message
/// (`Main.cpp:374`).
#[test]
fn clip_alpha_rect_rejects_a_source_that_is_not_a_layer() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4)]);

    let message = env.error("dst.clipAlphaRect(0, 0, new Object(), 0, 0, 2, 2)");
    assert!(
        message.contains("src must be Layer."),
        "the error must name the source: {message}"
    );
}

// ---------------------------------------------------------------------------
// overwrapRect
// ---------------------------------------------------------------------------

/// The source pixel is copied in full over the destination, and the pixels
/// whose alpha is below the threshold are skipped (`Main.cpp:554-560`).
#[test]
fn overwrap_rect_copies_the_source_pixel_over_the_destination() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 4, 4)]);
    env.run(
        "dst.fillRect(0, 0, 4, 4, 0x80112233); \
         src.fillRect(0, 0, 4, 4, 0xFF445566); \
         src.fillRect(0, 0, 1, 1, 0x00445566);",
    );

    env.run("dst.overwrapRect(0, 0, src, 0, 0, 2, 2);");

    // The source pixel at the origin has no alpha, so it is skipped
    // (Main.cpp:554-560).
    assert_eq!(
        pixel(&env, "dst", 0, 0),
        [0x11, 0x22, 0x33, 0x80],
        "a source pixel below the threshold must be skipped"
    );
    assert_eq!(
        pixel(&env, "dst", 1, 0),
        [0x44, 0x55, 0x66, 0xff],
        "the source pixel is copied in full"
    );
}

/// A threshold of zero lays every source pixel over the destination, the
/// transparent ones included (`Main.cpp:536-539`, `Main.cpp:554-560`).
#[test]
fn overwrap_rect_lays_every_source_pixel_over_with_a_threshold_of_zero() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("dst", 4, 4), ("src", 4, 4)]);
    env.run(
        "dst.fillRect(0, 0, 4, 4, 0x80112233); \
         src.fillRect(0, 0, 4, 4, 0x00445566);",
    );

    env.run("dst.overwrapRect(0, 0, src, 0, 0, 2, 2, 0);");

    assert_eq!(
        pixel(&env, "dst", 0, 0),
        [0x44, 0x55, 0x66, 0x00],
        "a threshold of zero lays every source pixel over"
    );
}

/// The member asks for no refresh, so the layer's `onPaint` handler does not
/// run (`Main.cpp:513-562`).
#[test]
fn overwrap_rect_asks_for_no_refresh() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    env.run(
        "var paints = 0; \
         class PaintLayer extends Layer { \
             function PaintLayer(w, parent) { super.Layer(w, parent); } \
             function onPaint() { paints++; } \
         } \
         var w = new Window(); \
         var dst = new PaintLayer(w, null); \
         var src = new Layer(w, null); \
         dst.setImageSize(4, 4); \
         src.setImageSize(4, 4);",
    );

    env.run("dst.overwrapRect(0, 0, src, 0, 0, 2, 2);");
    env.advance_transitions();
    assert_eq!(
        env.eval_int("paints"),
        0,
        "the member must not ask the layer for a repaint"
    );

    // A member that does ask for a refresh runs the handler.
    env.run("dst.fillAlpha();");
    env.advance_transitions();
    assert_eq!(
        env.eval_int("paints"),
        1,
        "a member that refreshes the layer runs its onPaint handler"
    );
}

// ---------------------------------------------------------------------------
// fillByProvince
// ---------------------------------------------------------------------------

/// The pixels whose province byte carries the index are filled with the
/// colour, and the others keep their value (`Main.cpp:596-608`).
#[test]
fn fill_by_province_fills_the_pixels_of_the_matching_province_byte() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 4, 1)]);
    env.run(
        "l.fillRect(0, 0, 4, 1, 0x80112233); \
         l.setProvincePixel(0, 0, 1); \
         l.setProvincePixel(1, 0, 1);",
    );

    env.run("l.fillByProvince(1, 0xFF00FF00);");

    assert_eq!(
        pixel(&env, "l", 0, 0),
        [0x00, 0xff, 0x00, 0xff],
        "a matching pixel takes the colour in full"
    );
    assert_eq!(
        pixel(&env, "l", 1, 0),
        [0x00, 0xff, 0x00, 0xff],
        "a matching pixel takes the colour in full"
    );
    assert_eq!(
        pixel(&env, "l", 2, 0),
        [0x11, 0x22, 0x33, 0x80],
        "a pixel with another province byte must not change"
    );
}

/// The index names the low byte of the argument, so a negative index wraps
/// around (`Main.cpp:571`).
#[test]
fn fill_by_province_takes_the_low_byte_of_the_index() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run(
        "l.fillRect(0, 0, 2, 1, 0x80112233); \
         l.setProvincePixel(0, 0, 255);",
    );

    env.run("l.fillByProvince(-1, 0xFF00FF00);");

    assert_eq!(
        pixel(&env, "l", 0, 0),
        [0x00, 0xff, 0x00, 0xff],
        "the index -1 names the province byte 255"
    );
}

/// A layer without a province plane has nothing to match, so the member
/// reports it (`Main.cpp:585-588`).
#[test]
fn fill_by_province_reports_a_layer_without_a_province_plane() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);

    let message = env.error("l.fillByProvince(1, 0xFF00FF00)");
    assert!(
        message.contains("no province image."),
        "the error must name the province plane: {message}"
    );
}

/// Fewer than two arguments is a bad parameter count (`Main.cpp:570`).
#[test]
fn fill_by_province_needs_two_arguments() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);

    let message = env.error("l.fillByProvince(1)");
    assert!(
        message.contains("Invalid argument count"),
        "the error must be the bad parameter count: {message}"
    );
}

// ---------------------------------------------------------------------------
// fillToProvince
// ---------------------------------------------------------------------------

/// The pixels of the colour are marked in the province plane, and the
/// bounding box of the marked pixels is answered (`Main.cpp:661-666`,
/// `Main.cpp:674-689`).
#[test]
fn fill_to_province_marks_the_pixels_of_the_colour_and_answers_their_box() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 4, 4)]);
    env.run(
        "l.fillRect(0, 0, 4, 4, 0xFF00FF00); \
         l.fillRect(2, 2, 2, 2, 0x80112233);",
    );

    let box_values = env
        .eval("l.fillToProvince(0x00FF00, 5)")
        .expect("the member answers the bounding box");
    let TjsValue::Object = box_values else {
        panic!("the member must answer an array, got {box_values:?}");
    };

    assert_eq!(
        province(&env, "l", 0, 0),
        5,
        "a pixel of the colour takes the province index"
    );
    assert_eq!(
        province(&env, "l", 1, 1),
        5,
        "a pixel of the colour takes the province index"
    );
    assert_eq!(
        province(&env, "l", 3, 3),
        0,
        "a pixel of another colour keeps the province value it had"
    );
}

/// The bounding box is the smallest and the largest marked column and row,
/// in the layer's own image coordinates (`Main.cpp:680-683`).
#[test]
fn fill_to_province_answers_the_bounding_box_of_the_marked_pixels() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 4, 4)]);
    env.run(
        "l.fillRect(0, 0, 4, 4, 0xFF000000); \
         l.fillRect(1, 1, 2, 2, 0xFF00FF00);",
    );

    let values = env
        .eval("l.fillToProvince(0x00FF00, 5)")
        .expect("the member answers the bounding box");
    let TjsValue::Object = values else {
        panic!("the member must answer an array, got {values:?}");
    };
    let left = env.eval_int("l.fillToProvince(0x00FF00, 5)[0]");
    let top = env.eval_int("l.fillToProvince(0x00FF00, 5)[1]");
    let width = env.eval_int("l.fillToProvince(0x00FF00, 5)[2]");
    let height = env.eval_int("l.fillToProvince(0x00FF00, 5)[3]");
    assert_eq!(
        (left, top, width, height),
        (1, 1, 2, 2),
        "the box must cover the marked pixels, got {values:?}"
    );
}

/// A colour that no pixel carries marks nothing, and the member answers
/// nothing (`Main.cpp:674-676`).
#[test]
fn fill_to_province_answers_nothing_when_no_pixel_matched() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 4, 4)]);
    env.run("l.fillRect(0, 0, 4, 4, 0xFF00FF00);");

    assert_eq!(
        env.eval("typeof l.fillToProvince(0x010203, 5)")
            .expect("the member answers"),
        tjs::TjsValue::String("void".to_string()),
        "the member must answer nothing when no pixel matched"
    );
}

/// A pixel whose alpha is below the threshold does not match, however close
/// its colour is (`Main.cpp:661`).
#[test]
fn fill_to_province_needs_the_alpha_to_reach_the_threshold() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run("l.fillRect(0, 0, 2, 1, 0x2000FF00);");

    assert_eq!(
        province(&env, "l", 0, 0),
        0,
        "a pixel below the threshold must not be marked"
    );
    env.run("l.fillToProvince(0x00FF00, 5, 0);");
    assert_eq!(
        province(&env, "l", 0, 0),
        5,
        "a threshold of zero matches every pixel of the colour"
    );
}

/// A negative colour does not match every colour: the value is truncated to
/// 32 bits, so its low 24 bits name white (`Main.cpp:620-622`).
#[test]
fn fill_to_province_compares_the_low_24_bits_of_the_colour() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run(
        "l.fillRect(0, 0, 1, 1, 0xFFFFFFFF); \
         l.fillRect(1, 0, 1, 1, 0xFF000000);",
    );

    env.run("l.fillToProvince(-1, 5);");

    assert_eq!(
        province(&env, "l", 0, 0),
        5,
        "the colour -1 is the white one, so the white pixel is marked"
    );
    assert_eq!(
        province(&env, "l", 1, 0),
        0,
        "the black pixel is not white, so it is not marked"
    );
}

/// Fewer than two arguments is a bad parameter count (`Main.cpp:619`).
#[test]
fn fill_to_province_needs_two_arguments() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);

    let message = env.error("l.fillToProvince(0x00FF00)");
    assert!(
        message.contains("Invalid argument count"),
        "the error must be the bad parameter count: {message}"
    );
}

/// The member writes the province plane of the layer it is called on, and the
/// plane it creates is the layer's own (`Main.cpp:640`, `Main.cpp:654`).
#[test]
fn fill_to_province_writes_the_layers_own_province_plane() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 2, 1)]);
    env.run("l.fillRect(0, 0, 2, 1, 0xFF00FF00);");

    env.run("l.fillToProvince(0x00FF00, 9);");

    let layer = layer(&env, "l");
    assert_eq!(
        layer.province_width, 2,
        "the province plane must have the layer's image width"
    );
    assert_eq!(
        province(&env, "l", 0, 0),
        9,
        "the marked pixel takes the index"
    );
}

/// The helper the tests use to reach a layer's pixels is the scene's own
/// state, so a change of the layer's image size is visible there too.
#[test]
fn the_layer_image_is_the_scene_bitmap() {
    let _lock = vm_lock();
    let env = common::Env::new();
    link(&env);
    make_layers(&env, &[("l", 3, 2)]);

    let layer = layer(&env, "l");
    let bitmap_id = layer.bitmap.expect("the layer has a main image");
    let bitmap = env.scene.bitmap(bitmap_id).expect("the bitmap exists");
    assert_eq!(
        (bitmap.width, bitmap.height),
        (3, 2),
        "the image must have the size the script asked for"
    );
}
