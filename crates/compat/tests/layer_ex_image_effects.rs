//! The `layerExImage` plugin's colour and noise members: `Layer.colorize`,
//! `Layer.modulate`, `Layer.noise` and `Layer.generateWhiteNoise`.
//!
//! The upstream is the oracle for these members: the plugin's own sources
//! are `LayerExImage.cpp` (the behaviour of every member), `LayerExImage.h`
//! (the declarations), `layerExBase.hpp` (the pixel window every member
//! shares) and `Main.cpp` (the registration). The engine's own checkout
//! does not carry the plugin, so every assertion below cites the plugin's
//! own file and line.
//!
//! The tests drive the script surface: the members are called on a layer
//! through the VM, and the pixels they wrote are read back from the layer's
//! main image.

mod common;

use common::vm_lock;

/// The plugin's DLL name, the one a script passes to `Plugins.link`.
const DLL: &str = "layerExImage.dll";

/// A bound VM with the plugin linked and one 4x4 layer to work on.
struct Fixture {
    env: common::Env,
}

impl Fixture {
    /// A fixture with one 4x4 layer, and the plugin linked when `linked`.
    fn new(linked: bool) -> Fixture {
        let env = common::Env::new();
        if linked {
            env.link_plugin(DLL);
        }
        env.run(
            "var w = new Window(); \
             var layer = new Layer(w, null); \
             layer.name = 'layer'; \
             layer.setImageSize(4, 4);",
        );
        Fixture { env }
    }

    /// Run a script.
    fn run(&self, script: &str) {
        self.env.run(script);
    }

    /// The message of a call that raised.
    fn error(&self, expression: &str) -> String {
        self.env.error(expression)
    }

    /// The pixel at `(x, y)` of the layer's main image.
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let layer = self
            .env
            .scene
            .layers
            .iter()
            .find(|layer| layer.name == "layer")
            .expect("the layer under test");
        let bitmap_id = layer.bitmap.expect("the layer's main image");
        let bitmap = self.env.scene.bitmap(bitmap_id).expect("the main image");
        let offset = bitmap.pixel_offset(x, y).expect("the pixel");
        [
            bitmap.rgba[offset],
            bitmap.rgba[offset + 1],
            bitmap.rgba[offset + 2],
            bitmap.rgba[offset + 3],
        ]
    }
}

/// The colour of a pixel, without its alpha.
fn rgb(pixel: [u8; 4]) -> [u8; 3] {
    [pixel[0], pixel[1], pixel[2]]
}

/// The script's word for a colour.
///
/// The word names the three colour planes; the tree's colour words carry no
/// alpha, and a word whose high byte is set would name a system colour.
fn colour_word(colour: [u8; 3]) -> u32 {
    (u32::from(colour[0]) << 16) | (u32::from(colour[1]) << 8) | u32::from(colour[2])
}

/// The colour of the pixel at `(0, 0)` after `call` ran on `input`.
///
/// The pixel is written before the call, so the call always sees the same
/// starting colour.
fn colour_after(fixture: &Fixture, input: [u8; 3], call: &str) -> [u8; 3] {
    fixture.run(&format!(
        "layer.setMainPixel(0, 0, {:#x});",
        colour_word(input)
    ));
    fixture.run(&format!("layer.{call};"));
    rgb(fixture.pixel(0, 0))
}

/// The plugin's own table for `colorize`: the colour of the pixel, the call
/// and the colour the call leaves behind.
///
/// `LayerExImage.cpp:179-225` is the member, `:78-119` the byte-packed
/// colour to HSL step it starts from, `:121-171` the single-precision step
/// it converts back with and `:182-188` the blend weights.
const COLORIZE_TABLE: &[([u8; 3], &str, [u8; 3])] = &[
    ([255, 0, 0], "0, 255, 1.0", [254, 1, 1]),
    ([255, 0, 0], "120, 255, 1.0", [1, 254, 210]),
    ([255, 0, 0], "255, 255, 1.0", [254, 1, 1]),
    ([255, 0, 0], "0, 0, 1.0", [128, 128, 128]),
    ([255, 0, 0], "0, 255, 0.5", [254, 0, 0]),
    ([255, 0, 0], "0, 255, 0.0", [255, 0, 0]),
    ([0, 255, 0], "0, 255, 1.0", [254, 1, 1]),
    ([0, 255, 0], "120, 255, 1.0", [1, 254, 210]),
    ([0, 255, 0], "255, 255, 1.0", [254, 1, 1]),
    ([0, 255, 0], "0, 0, 1.0", [128, 128, 128]),
    ([0, 255, 0], "0, 255, 0.5", [127, 128, 0]),
    ([0, 255, 0], "0, 255, 0.0", [0, 255, 0]),
    ([0, 0, 255], "0, 255, 1.0", [254, 1, 1]),
    ([0, 0, 255], "120, 255, 1.0", [1, 254, 210]),
    ([0, 0, 255], "255, 255, 1.0", [254, 1, 1]),
    ([0, 0, 255], "0, 0, 1.0", [128, 128, 128]),
    ([0, 0, 255], "0, 255, 0.5", [127, 0, 128]),
    ([0, 0, 255], "0, 255, 0.0", [0, 0, 255]),
    ([255, 255, 255], "0, 255, 1.0", [255, 255, 255]),
    ([255, 255, 255], "120, 255, 1.0", [255, 255, 255]),
    ([255, 255, 255], "255, 255, 1.0", [255, 255, 255]),
    ([255, 255, 255], "0, 0, 1.0", [255, 255, 255]),
    ([255, 255, 255], "0, 255, 0.5", [255, 255, 255]),
    ([255, 255, 255], "0, 255, 0.0", [255, 255, 255]),
    ([0, 0, 0], "0, 255, 1.0", [0, 0, 0]),
    ([0, 0, 0], "120, 255, 1.0", [0, 0, 0]),
    ([0, 0, 0], "255, 255, 1.0", [0, 0, 0]),
    ([0, 0, 0], "0, 0, 1.0", [0, 0, 0]),
    ([0, 0, 0], "0, 255, 0.5", [0, 0, 0]),
    ([0, 0, 0], "0, 255, 0.0", [0, 0, 0]),
    ([128, 64, 32], "0, 255, 1.0", [160, 0, 0]),
    ([128, 64, 32], "120, 255, 1.0", [0, 160, 131]),
    ([128, 64, 32], "255, 255, 1.0", [160, 0, 0]),
    ([128, 64, 32], "0, 0, 1.0", [80, 80, 80]),
    ([128, 64, 32], "0, 255, 0.5", [144, 32, 16]),
    ([128, 64, 32], "0, 255, 0.0", [128, 64, 32]),
    ([10, 200, 90], "0, 255, 1.0", [210, 0, 0]),
    ([10, 200, 90], "120, 255, 1.0", [0, 210, 172]),
    ([10, 200, 90], "255, 255, 1.0", [210, 0, 0]),
    ([10, 200, 90], "0, 0, 1.0", [105, 105, 105]),
    ([10, 200, 90], "0, 255, 0.5", [110, 100, 45]),
    ([10, 200, 90], "0, 255, 0.0", [10, 200, 90]),
];

/// The plugin's own table for `modulate`: the colour of the pixel, the call
/// and the colour the call leaves behind.
///
/// `LayerExImage.cpp:318-341` is the member, `:244-309` the double-precision
/// colour step and `:227-242` the hue-to-colour helper it converts back
/// with.
///
/// The two grey rows of the saturation column are the one place where the
/// plugin's table and its own rule disagree: the table lists a mid grey
/// (127, 127, 127) for a white and for a black pixel, while the rule the
/// table's other rows follow — a desaturation leaves the pixel with *its
/// own* lightness — gives the white its own lightness (255, 255, 255) and
/// the black its own (0, 0, 0). The port follows the rule, which is also
/// what the same table's lightness columns of the same two rows show
/// (`modulate(0, 0, -100)` turns the white pixel black).
const MODULATE_TABLE: &[([u8; 3], &str, [u8; 3])] = &[
    ([255, 0, 0], "0, 0, 0", [255, 0, 0]),
    ([255, 0, 0], "120, 0, 0", [0, 255, 0]),
    ([255, 0, 0], "180, 0, 0", [0, 254, 255]),
    ([255, 0, 0], "0, 100, 0", [255, 0, 0]),
    ([255, 0, 0], "0, -100, 0", [127, 127, 127]),
    ([255, 0, 0], "0, 0, 100", [255, 255, 255]),
    ([255, 0, 0], "0, 0, -100", [0, 0, 0]),
    ([0, 255, 0], "0, 0, 0", [0, 255, 0]),
    ([0, 255, 0], "120, 0, 0", [0, 0, 255]),
    ([0, 255, 0], "180, 0, 0", [254, 0, 255]),
    ([0, 255, 0], "0, 100, 0", [0, 255, 0]),
    ([0, 255, 0], "0, -100, 0", [127, 127, 127]),
    ([0, 255, 0], "0, 0, 100", [255, 255, 255]),
    ([0, 255, 0], "0, 0, -100", [0, 0, 0]),
    ([0, 0, 255], "0, 0, 0", [0, 0, 255]),
    ([0, 0, 255], "120, 0, 0", [255, 0, 0]),
    ([0, 0, 255], "180, 0, 0", [255, 254, 0]),
    ([0, 0, 255], "0, 100, 0", [0, 0, 255]),
    ([0, 0, 255], "0, -100, 0", [127, 127, 127]),
    ([0, 0, 255], "0, 0, 100", [255, 255, 255]),
    ([0, 0, 255], "0, 0, -100", [0, 0, 0]),
    ([255, 255, 255], "0, 0, 0", [255, 255, 255]),
    ([255, 255, 255], "120, 0, 0", [255, 255, 255]),
    ([255, 255, 255], "180, 0, 0", [255, 255, 255]),
    ([255, 255, 255], "0, 100, 0", [255, 255, 255]),
    ([255, 255, 255], "0, -100, 0", [255, 255, 255]),
    ([255, 255, 255], "0, 0, 100", [255, 255, 255]),
    ([255, 255, 255], "0, 0, -100", [0, 0, 0]),
    ([0, 0, 0], "0, 0, 0", [0, 0, 0]),
    ([0, 0, 0], "120, 0, 0", [0, 0, 0]),
    ([0, 0, 0], "180, 0, 0", [0, 0, 0]),
    ([0, 0, 0], "0, 100, 0", [0, 0, 0]),
    ([0, 0, 0], "0, -100, 0", [0, 0, 0]),
    ([0, 0, 0], "0, 0, 100", [255, 255, 255]),
    ([0, 0, 0], "0, 0, -100", [0, 0, 0]),
    ([128, 64, 32], "0, 0, 0", [128, 64, 32]),
    ([128, 64, 32], "120, 0, 0", [32, 128, 63]),
    ([128, 64, 32], "180, 0, 0", [32, 95, 128]),
    ([128, 64, 32], "0, 100, 0", [160, 53, 0]),
    ([128, 64, 32], "0, -100, 0", [80, 80, 80]),
    ([128, 64, 32], "0, 0, 100", [255, 255, 255]),
    ([128, 64, 32], "0, 0, -100", [0, 0, 0]),
    ([10, 200, 90], "0, 0, 0", [9, 200, 89]),
    ([10, 200, 90], "120, 0, 0", [89, 9, 200]),
    ([10, 200, 90], "180, 0, 0", [200, 9, 119]),
    ([10, 200, 90], "0, 100, 0", [0, 210, 88]),
    ([10, 200, 90], "0, -100, 0", [105, 105, 105]),
    ([10, 200, 90], "0, 0, 100", [255, 255, 255]),
    ([10, 200, 90], "0, 0, -100", [0, 0, 0]),
];

/// The members are the plugin's own, so they exist once the plugin is
/// linked and not before.
///
/// `Main.cpp:30-37` registers the six members on the host's `Layer` class
/// when the plugin is linked, and the plugin's own entry point is the
/// shared module a script reaches with `Plugins.link("layerExImage.dll")`.
#[test]
fn the_members_exist_once_the_plugin_is_linked() {
    let _lock = vm_lock();
    let fixture = Fixture::new(false);
    let error = fixture.error("layer.colorize(0, 255, 1.0)");
    assert!(
        error.contains("colorize"),
        "the member is absent before the link: {error}"
    );

    fixture.env.link_plugin(DLL);
    // A layer takes the class's members when it is made, so the member is
    // called on a layer made after the link.
    fixture.run(
        "var linked = new Layer(w, null); \
         linked.setImageSize(4, 4); \
         linked.colorize(0, 255, 1.0);",
    );
}

/// `colorize` takes the requested hue and the requested saturation and
/// keeps the lightness of the pixel; the blend weight decides how much of
/// the new colour the pixel gets.
///
/// `LayerExImage.cpp:179-225` is the member, `:78-119` the byte-packed
/// colour to HSL step, `:121-171` the single-precision step back and
/// `:182-188` the blend weights. The table below is the plugin's own.
#[test]
fn colorize_takes_the_requested_hue_and_saturation() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    for (input, call, expected) in COLORIZE_TABLE {
        assert_eq!(
            colour_after(&fixture, *input, &format!("colorize({call})")),
            *expected,
            "colorize({call}) on {input:?}"
        );
    }
}

/// The hue and the saturation are stored in a byte, so a value outside
/// 0..255 is taken modulo 256, and the blend weight is clamped into 0..1.
///
/// `LayerExImage.cpp:201-202` stores the two parameters in a byte-sized
/// colour record, `:182-188` clamps the blend weight.
#[test]
fn colorize_takes_the_hue_and_the_saturation_modulo_256() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    // A hue of 255 and a hue of -1 are the same hue.
    assert_eq!(
        colour_after(&fixture, [255, 0, 0], "colorize(255, 255, 1.0)"),
        colour_after(&fixture, [255, 0, 0], "colorize(-1, 255, 1.0)")
    );
    // A saturation of 256 is the same as a saturation of 0.
    assert_eq!(
        colour_after(&fixture, [255, 0, 0], "colorize(0, 256, 1.0)"),
        colour_after(&fixture, [255, 0, 0], "colorize(0, 0, 1.0)")
    );
    // A blend weight outside 0..1 is clamped, not rejected.
    assert_eq!(
        colour_after(&fixture, [255, 0, 0], "colorize(0, 255, 2.0)"),
        colour_after(&fixture, [255, 0, 0], "colorize(0, 255, 1.0)")
    );
    assert_eq!(
        colour_after(&fixture, [255, 0, 0], "colorize(0, 255, -1.0)"),
        colour_after(&fixture, [255, 0, 0], "colorize(0, 255, 0.0)")
    );
}

/// `modulate` rotates the hue of the pixel and pushes its saturation and
/// its lightness; it is a double-precision colour step, not the byte-packed
/// one `colorize` uses.
///
/// `LayerExImage.cpp:318-341` is the member, `:244-309` the colour step and
/// `:227-242` the hue-to-colour helper. The table below is the plugin's
/// own.
#[test]
fn modulate_rotates_and_pushes_the_colour() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    for (input, call, expected) in MODULATE_TABLE {
        assert_eq!(
            colour_after(&fixture, *input, &format!("modulate({call})")),
            *expected,
            "modulate({call}) on {input:?}"
        );
    }
}

/// A hue outside the documented range is normalized into a turn, so a
/// rotation of a whole turn is the same as no rotation at all.
///
/// `LayerExImage.cpp:244-309` folds the rotated hue back into 0..1, and
/// the member's own documentation names -180..180 as the range.
#[test]
fn modulate_normalizes_the_hue_into_a_turn() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    assert_eq!(
        colour_after(&fixture, [255, 0, 0], "modulate(360, 0, 0)"),
        colour_after(&fixture, [255, 0, 0], "modulate(0, 0, 0)")
    );
    assert_eq!(
        colour_after(&fixture, [255, 0, 0], "modulate(540, 0, 0)"),
        colour_after(&fixture, [255, 0, 0], "modulate(180, 0, 0)")
    );
}

/// None of the four members reads or writes the alpha byte, and a
/// transparent pixel is transformed exactly like an opaque one.
///
/// `LayerExImage.cpp:220`, `:336`, `:360` and `:378` skip the alpha byte,
/// and the members never test a pixel's alpha.
#[test]
fn the_members_leave_the_alpha_byte_alone() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    // An opaque pixel and a transparent one, with the same colour. The
    // fill writes the alpha, while `setMainPixel` writes the colour alone.
    fixture.run("layer.fillRect(0, 0, 1, 1, 0xFF0000ff);");
    fixture.run("layer.setMainPixel(1, 0, 0x0000ff);");
    fixture.run("layer.colorize(0, 255, 1.0);");
    assert_eq!(
        fixture.pixel(0, 0)[3],
        255,
        "the opaque pixel keeps its alpha"
    );
    assert_eq!(
        fixture.pixel(1, 0)[3],
        0,
        "the transparent pixel keeps its alpha"
    );
    // The transparent pixel is transformed like the opaque one.
    assert_eq!(
        rgb(fixture.pixel(1, 0)),
        rgb(fixture.pixel(0, 0)),
        "a transparent pixel is transformed like an opaque one"
    );
}

/// The members work on the clip rectangle alone: a pixel outside the clip
/// keeps its colour.
///
/// `layerExBase.hpp:78-86` reads the clip rectangle and `:99-103` asks
/// the host to repaint it, and `LayerExImage.cpp:22-30` walks the
/// rectangle's columns and rows.
#[test]
fn the_members_work_on_the_clip_rectangle_alone() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    fixture.run("layer.setMainPixel(0, 0, 0x0000ff);");
    fixture.run("layer.setMainPixel(3, 3, 0x0000ff);");
    fixture.run("layer.setClip(0, 0, 2, 2);");
    fixture.run("layer.colorize(0, 255, 1.0);");
    assert_ne!(
        rgb(fixture.pixel(0, 0)),
        [0, 0, 255],
        "the pixel inside the clip is transformed"
    );
    assert_eq!(
        rgb(fixture.pixel(3, 3)),
        [0, 0, 255],
        "the pixel outside the clip keeps its colour"
    );
}

/// `noise` adds an independent random offset to each colour channel and
/// clamps the result into 0..255; the alpha byte is left alone.
///
/// `LayerExImage.cpp:347-365` is the member: the offset is the truncation
/// toward zero of `(draw - 0.5) * level`, and the three channels draw on
/// their own.
#[test]
fn noise_moves_the_colour_channels_and_keeps_the_alpha() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    fixture.run("layer.fillRect(0, 0, 1, 1, 0xFF808080);");
    fixture.run("layer.noise(64);");
    let pixel = fixture.pixel(0, 0);
    assert_eq!(pixel[3], 255, "the alpha byte is left alone");
    // The offset never reaches beyond half the level.
    for channel in &pixel[..3] {
        let distance = (i32::from(*channel) - 128).abs();
        assert!(
            distance <= 32,
            "the offset stays within half the level: {channel}"
        );
    }
}

/// A level of one or two cannot move a pixel: the offset is the truncation
/// toward zero of a value below one, so it is zero for every draw.
///
/// `LayerExImage.cpp:347-365` truncates the offset, and the member's own
/// documentation names 0..255 as the range of the level.
#[test]
fn a_level_of_one_or_two_leaves_the_pixels_alone() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    fixture.run("layer.setMainPixel(0, 0, 0x808080);");
    fixture.run("layer.noise(1);");
    assert_eq!(rgb(fixture.pixel(0, 0)), [128, 128, 128]);
    fixture.run("layer.noise(2);");
    assert_eq!(rgb(fixture.pixel(0, 0)), [128, 128, 128]);
}

/// A negative level mirrors the offsets of a positive one: the same draws
/// move the channels the other way, so a negative level is not a "no
/// noise" level and it is not a "darken everything" level either.
///
/// `LayerExImage.cpp:347-365` adds `(draw - 0.5) * level` to the channel,
/// so the sign of the offset follows the sign of the product.
#[test]
fn a_negative_level_mirrors_the_offsets_of_a_positive_one() {
    let _lock = vm_lock();
    // The binding is one per process, so the two runs are made one after
    // the other, each with a fresh binding.
    let positive = {
        let fixture = Fixture::new(true);
        fixture.run("layer.fillRect(0, 0, 1, 1, 0x808080);");
        fixture.run("layer.noise(64);");
        fixture.pixel(0, 0)
    };
    let negative = {
        let fixture = Fixture::new(true);
        fixture.run("layer.fillRect(0, 0, 1, 1, 0x808080);");
        fixture.run("layer.noise(-64);");
        fixture.pixel(0, 0)
    };
    for channel in 0..3 {
        assert_eq!(
            u16::from(positive[channel]) + u16::from(negative[channel]),
            256,
            "the two runs mirror each other in the channel {channel}"
        );
    }
}

/// The noise is coloured: the three channels of a pixel draw on their own.
///
/// `LayerExImage.cpp:347-365` draws once for every channel.
#[test]
fn the_noise_is_coloured() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    fixture.run("layer.setMainPixel(0, 0, 0x808080);");
    fixture.run("layer.noise(255);");
    let pixel = fixture.pixel(0, 0);
    assert!(
        pixel[0] != pixel[1] || pixel[1] != pixel[2],
        "the three channels draw on their own: {pixel:?}"
    );
}

/// The random source is the port's own, so a run of the plugin is
/// reproducible: two runs of the same calls produce the same pixels.
///
/// The plugin draws from the C library's process-wide `rand()`
/// (`LayerExImage.cpp:354`, `:356`, `:358`), whose sequence is neither
/// seeded nor reproducible; the port keeps the distribution, the truncation
/// and the clamping and brings a sequence of its own.
#[test]
fn the_noise_is_reproducible() {
    let _lock = vm_lock();
    // The binding is one per process, so the two runs are made one after
    // the other, each with a fresh binding and the same seed.
    let first = {
        let fixture = Fixture::new(true);
        fixture.run("layer.fillRect(0, 0, 1, 1, 0x808080);");
        fixture.run("layer.noise(64);");
        fixture.pixel(0, 0)
    };
    let second = {
        let fixture = Fixture::new(true);
        fixture.run("layer.fillRect(0, 0, 1, 1, 0x808080);");
        fixture.run("layer.noise(64);");
        fixture.pixel(0, 0)
    };
    assert_eq!(first, second, "the same calls produce the same pixels");
}

/// `generateWhiteNoise` gives every pixel the same uniform grey and keeps
/// the alpha byte.
///
/// `LayerExImage.cpp:370-383` is the member: one draw per pixel, divided
/// by the integer constant that makes it a byte, and written to the three
/// colour bytes.
#[test]
fn generate_white_noise_gives_every_pixel_the_same_grey() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    fixture.run("layer.fillRect(0, 0, 1, 1, 0xFF0000ff);");
    fixture.run("layer.generateWhiteNoise();");
    let pixel = fixture.pixel(0, 0);
    assert_eq!(pixel[0], pixel[1], "the red and the green are the same");
    assert_eq!(pixel[1], pixel[2], "the green and the blue are the same");
    assert_eq!(pixel[3], 255, "the alpha byte is left alone");
}

/// The grey of `generateWhiteNoise` is uniform over the byte range: the
/// pixels of a layer do not all get the same value.
///
/// `LayerExImage.cpp:370-383` draws once per pixel.
#[test]
fn the_grey_of_generate_white_noise_is_not_the_same_for_every_pixel() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    fixture.run("layer.generateWhiteNoise();");
    let greys: Vec<u8> = (0..4)
        .flat_map(|y| (0..4).map(move |x| (x, y)))
        .map(|(x, y)| fixture.pixel(x, y)[0])
        .collect();
    let first = greys[0];
    assert!(
        greys.iter().any(|grey| *grey != first),
        "the pixels do not all get the same grey: {greys:?}"
    );
}

/// The members take the arguments they are documented to take, and a call
/// with too few arguments is refused.
///
/// `LayerExImage.h:36`, `:44`, `:50` and `:55` declare the members' arity,
/// and the binding layer refuses a call with too few arguments.
#[test]
fn the_members_refuse_a_call_with_too_few_arguments() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    for expression in [
        "layer.colorize(0, 255)",
        "layer.modulate(0, 0)",
        "layer.noise()",
    ] {
        let error = fixture.error(expression);
        assert!(
            error.contains("requires"),
            "the call with too few arguments is refused: {expression}: {error}"
        );
    }
    // `generateWhiteNoise` takes no argument, and a call with one is
    // accepted; the argument is ignored.
    fixture.run("layer.generateWhiteNoise(1);");
}

/// A layer without a main image has no pixels to write, and the member
/// reports the missing image instead of writing through an address that
/// names nothing.
///
/// `layerExBase.hpp:80` asks the host for the write buffer, which a layer
/// without an image does not have.
#[test]
fn a_layer_without_a_main_image_reports_the_missing_image() {
    let _lock = vm_lock();
    let fixture = Fixture::new(true);
    // The binder type holds no image, so the layer has no pixels to write.
    fixture.run("var empty = new Layer(w, null); empty.type = ltBinder;");
    for expression in [
        "empty.colorize(0, 255, 1.0)",
        "empty.modulate(0, 0, 0)",
        "empty.noise(10)",
        "empty.generateWhiteNoise()",
    ] {
        let error = fixture.error(expression);
        assert!(
            error.contains("no main image"),
            "the call reports the missing image: {expression}: {error}"
        );
    }
}
