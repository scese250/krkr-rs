//! Integration tests for the `layerExLongExposure` plugin: the five members
//! a `Layer` class gains when a script links `layerExLongExposure.dll`.
//!
//! The plugin publishes no class of its own; it attaches `initExposure`,
//! `snapExposure`, `statExposure`, `copyExposure` and `termExposure` to the
//! engine's own `Layer` class (`upstream Main.cpp:229-236`), so the members
//! exist only after the plugin is linked.
//!
//! The behaviour oracle is the plugin's own source and manual:
//!
//! * `upstream Main.cpp:15-22` — `initExposure`
//! * `upstream Main.cpp:24-47` — `snapExposure`
//! * `upstream Main.cpp:87-109` — `statExposure`
//! * `upstream Main.cpp:111-157` — `copyExposure`
//! * `upstream Main.cpp:159-163` — `termExposure`
//! * `upstream Main.cpp:49-74` — the normalisation arithmetic
//! * `upstream manual.tjs:8-45` — the members the plugin documents
//!
//! The tests drive the script surface: they link the plugin, create a layer
//! with a main image and call the members.

mod common;

use common::vm_lock;
use tjs::TjsValue;

/// A window and one layer with a 4x4 main image, named `exposure`.
fn make_layer(env: &common::Env) {
    env.run(
        "var w = new Window(); \
         var exposure = new Layer(w, null); \
         exposure.name = 'exposure'; \
         exposure.setImageSize(4, 4);",
    );
}

/// A window and two layers with a 4x4 main image each, named `first` and
/// `second`.
fn make_two_layers(env: &common::Env) {
    env.run(
        "var w = new Window(); \
         var first = new Layer(w, null); \
         var second = new Layer(w, null); \
         first.name = 'first'; \
         second.name = 'second'; \
         first.setImageSize(4, 4); \
         second.setImageSize(4, 4);",
    );
}

/// The straight-alpha RGBA pixel at `(x, y)` of the named layer's main image.
fn pixel(env: &common::Env, name: &str, x: u32, y: u32) -> [u8; 4] {
    let layer = env
        .scene
        .layers
        .iter()
        .find(|layer| layer.name == name)
        .unwrap_or_else(|| panic!("no scene layer named {name:?}"));
    let bitmap_id = layer.bitmap.expect("the layer's main image");
    let bitmap = env.scene.bitmap(bitmap_id).expect("the layer's bitmap");
    let offset = bitmap.pixel_offset(x, y).expect("the pixel is in range");
    [
        bitmap.rgba[offset],
        bitmap.rgba[offset + 1],
        bitmap.rgba[offset + 2],
        bitmap.rgba[offset + 3],
    ]
}

/// The error a call raises, checked against the message the plugin uses.
fn assert_error(env: &common::Env, call: &str, message: &str) {
    let error = env.error(call);
    assert!(
        error.contains(message),
        "{call} raised {error:?}, expected a message containing {message:?}"
    );
}

#[test]
fn the_members_appear_only_when_the_plugin_is_linked() {
    let _lock = vm_lock();
    let env = common::Env::new();
    // A layer created before the link does not carry the members: the native
    // class copies its members onto every instance it creates
    // (`upstream Main.cpp:229-236`).
    env.run("var w = new Window(); var early = new Layer(w, null);");
    assert_eq!(
        env.eval("typeof early.snapExposure"),
        Ok(TjsValue::String("undefined".to_string()))
    );
    assert_error(&env, "early.initExposure()", "does not exist");
    env.link_plugin("layerExLongExposure.dll");
    // A layer created after the link carries them, and its image is usable.
    env.run("var late = new Layer(w, null); late.setImageSize(4, 4);");
    assert_eq!(
        env.eval("typeof late.snapExposure"),
        Ok(TjsValue::String("Object".to_string()))
    );
    env.run("late.initExposure();");
}

#[test]
fn init_exposure_refuses_a_layer_without_an_image() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    // A layer whose main image is released has no image to accumulate
    // (`upstream Main.cpp:17-18`).
    env.run("var w = new Window(); var bare = new Layer(w, null); bare.hasImage = false;");
    assert_error(
        &env,
        "bare.initExposure()",
        "LongExposure.init: invalid layer image",
    );
}

#[test]
fn init_exposure_refuses_a_layer_without_an_accumulation() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    // The three members that need an accumulation refuse without one
    // (`upstream Main.cpp:25-26`, `upstream Main.cpp:88-89`,
    // `upstream Main.cpp:112-113`).
    assert_error(
        &env,
        "exposure.snapExposure()",
        "LongExposure.snap: not initialized",
    );
    assert_error(
        &env,
        "exposure.statExposure()",
        "LongExposure.stat: not initialized",
    );
    assert_error(
        &env,
        "exposure.copyExposure(%[])",
        "LongExposure.copy: not initialized",
    );
}

#[test]
fn the_statistics_are_the_accumulated_sums() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    // An opaque blue image, snapped twice: every counter holds twice the
    // byte it was given (`upstream Main.cpp:37-46`).
    env.run(
        "exposure.fillRect(0, 0, 4, 4, 0xFF0000FF); \
         exposure.initExposure(); \
         exposure.snapExposure(); \
         exposure.snapExposure();",
    );
    // The blue counter holds 2 * 255, the alpha counter the same, and the
    // other two hold zero (`upstream Main.cpp:95-107`).
    assert_eq!(env.eval_int("exposure.statExposure().min_b"), 510);
    assert_eq!(env.eval_int("exposure.statExposure().max_b"), 510);
    assert_eq!(env.eval_int("exposure.statExposure().min_r"), 0);
    assert_eq!(env.eval_int("exposure.statExposure().max_r"), 0);
    assert_eq!(env.eval_int("exposure.statExposure().min_g"), 0);
    assert_eq!(env.eval_int("exposure.statExposure().max_g"), 0);
    assert_eq!(env.eval_int("exposure.statExposure().min_a"), 510);
    assert_eq!(env.eval_int("exposure.statExposure().max_a"), 510);
}

#[test]
fn a_freshly_initialised_layer_reports_zero() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    // The counters start at zero (`upstream Main.cpp:19-21`).
    env.run("exposure.initExposure();");
    assert_eq!(env.eval_int("exposure.statExposure().min_r"), 0);
    assert_eq!(env.eval_int("exposure.statExposure().max_a"), 0);
}

#[test]
fn init_exposure_twice_restarts_the_accumulation() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    // The second initialisation throws the first buffer away and starts over
    // (`upstream Main.cpp:16`).
    env.run(
        "exposure.fillRect(0, 0, 4, 4, 0xFF0000FF); \
         exposure.initExposure(); \
         exposure.snapExposure(); \
         exposure.initExposure();",
    );
    assert_eq!(env.eval_int("exposure.statExposure().max_b"), 0);
}

#[test]
fn the_copy_stretches_the_accumulation_over_the_channel_range() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    // The left column is opaque red and the right one opaque cyan, so every
    // colour channel's range runs from zero to the byte it holds.
    env.run(
        "exposure.fillRect(0, 0, 1, 4, 0xFFFF0000); \
         exposure.fillRect(1, 0, 1, 4, 0xFF00FFFF); \
         exposure.initExposure(); \
         exposure.snapExposure();",
    );
    // The copy takes the statistics of the accumulation itself when the
    // argument is void (`upstream Main.cpp:127-130`).
    env.run("exposure.copyExposure(void);");
    // Each channel is stretched over its own range, and the alpha — whose
    // range has no spread — is written as 255
    // (`upstream Main.cpp:147-156`).
    assert_eq!(pixel(&env, "exposure", 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&env, "exposure", 1, 0), [0, 255, 255, 255]);
}

#[test]
fn the_copy_uses_the_ranges_the_argument_carries() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run(
        "exposure.fillRect(0, 0, 1, 4, 0xFFFF0000); \
         exposure.fillRect(1, 0, 1, 4, 0xFF00FFFF); \
         exposure.initExposure(); \
         exposure.snapExposure(); \
         exposure.snapExposure();",
    );
    // The red counter holds 510 at the red column and zero at the cyan one,
    // so a range of 0…510 writes the top of the range there. The green
    // channel keeps the pair (0, 0), which has no range to stretch and
    // writes 255 (`upstream Main.cpp:54`, `upstream Main.cpp:79-82`).
    env.run(
        "exposure.copyExposure(%[ \
           min_r:0, max_r:510, \
           min_g:0, max_g:0, \
           min_b:0, max_b:510, \
           min_a:0, max_a:0]);",
    );
    assert_eq!(pixel(&env, "exposure", 0, 0), [255, 255, 0, 255]);
    assert_eq!(pixel(&env, "exposure", 1, 0), [0, 255, 255, 255]);
}

#[test]
fn a_missing_member_reads_as_zero() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run(
        "exposure.fillRect(0, 0, 4, 4, 0xFFFF0000); \
         exposure.initExposure(); \
         exposure.snapExposure();",
    );
    // Only the lower bound of the red channel is given, so the upper bound
    // reads as zero and the pair has no range to stretch: the whole image is
    // written as 255 (`upstream Main.cpp:131-145`).
    env.run("exposure.copyExposure(%[min_r:100]);");
    assert_eq!(pixel(&env, "exposure", 0, 0), [255, 255, 255, 255]);
    assert_eq!(pixel(&env, "exposure", 3, 3), [255, 255, 255, 255]);
}

#[test]
fn an_argument_that_is_not_an_object_writes_255() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run(
        "exposure.fillRect(0, 0, 4, 4, 0xFFFF0000); \
         exposure.initExposure(); \
         exposure.snapExposure();",
    );
    // An integer, a real, a string and a null are no object, so no pair is
    // taken from them and the built-in pair stays: every byte is 255
    // (`upstream Main.cpp:126-146`).
    for argument in ["1", "0.5", "'text'", "null"] {
        env.run(&format!("exposure.copyExposure({argument});"));
        assert_eq!(
            pixel(&env, "exposure", 1, 1),
            [255, 255, 255, 255],
            "the argument {argument} did not write an opaque white image"
        );
    }
}

#[test]
fn the_copy_needs_one_argument() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run("exposure.initExposure();");
    // The member takes one argument, and the call is refused before the
    // member runs (`upstream Main.cpp:234`).
    assert_error(&env, "exposure.copyExposure()", "Invalid argument count");
}

#[test]
fn a_member_that_is_not_an_integer_is_refused() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run("exposure.initExposure();");
    // A member that holds an object cannot become an integer, and the
    // engine's conversion error is raised when it is read
    // (`upstream Main.cpp:131-145`).
    assert_error(
        &env,
        "exposure.copyExposure(%[min_r:%[]])",
        "Cannot convert the variable type ((object) to int)",
    );
}

#[test]
fn a_snap_after_a_resize_is_refused() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run(
        "exposure.fillRect(0, 0, 4, 4, 0xFF0000FF); \
         exposure.initExposure(); \
         exposure.snapExposure(); \
         exposure.setImageSize(8, 8);",
    );
    // The image size no longer matches the one the accumulation was created
    // for, and the check precedes the addition (`upstream Main.cpp:30-31`).
    assert_error(
        &env,
        "exposure.snapExposure()",
        "LongExposure.snap: invalid layer size",
    );
    // The accumulation is untouched: it still holds the one snap.
    assert_eq!(env.eval_int("exposure.statExposure().max_b"), 255);
}

#[test]
fn a_copy_after_a_resize_is_refused() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run(
        "exposure.initExposure(); \
         exposure.snapExposure(); \
         exposure.setImageSize(8, 8);",
    );
    assert_error(
        &env,
        "exposure.copyExposure(void)",
        "LongExposure.copy: invalid layer size",
    );
}

#[test]
fn term_exposure_releases_the_accumulation() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run("exposure.initExposure(); exposure.snapExposure();");
    // The release is not an error, and it is not an error to release twice
    // (`upstream Main.cpp:159-163`).
    env.run("exposure.termExposure();");
    env.run("exposure.termExposure();");
    assert_error(
        &env,
        "exposure.statExposure()",
        "LongExposure.stat: not initialized",
    );
    // A fresh initialisation starts a new accumulation.
    env.run("exposure.initExposure();");
    assert_eq!(env.eval_int("exposure.statExposure().max_b"), 0);
}

#[test]
fn the_accumulation_belongs_to_one_layer() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_two_layers(&env);
    // Each layer carries its own accumulation (`upstream Main.cpp:219-228`).
    env.run(
        "first.fillRect(0, 0, 4, 4, 0xFF0000FF); \
         second.fillRect(0, 0, 4, 4, 0xFFFF0000); \
         first.initExposure(); \
         second.initExposure(); \
         first.snapExposure(); \
         second.snapExposure(); \
         second.snapExposure();",
    );
    assert_eq!(env.eval_int("first.statExposure().max_b"), 255);
    assert_eq!(env.eval_int("first.statExposure().max_r"), 0);
    assert_eq!(env.eval_int("second.statExposure().max_r"), 510);
    assert_eq!(env.eval_int("second.statExposure().max_b"), 0);
}

#[test]
fn the_province_plane_is_never_touched() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run("exposure.setProvincePixel(0, 0, 7);");
    env.run(
        "exposure.fillRect(0, 0, 4, 4, 0xFF0000FF); \
         exposure.initExposure(); \
         exposure.snapExposure(); \
         exposure.copyExposure(void);",
    );
    // The copy writes the main image alone; the province plane keeps its
    // byte (`upstream Main.cpp:147-156`).
    assert_eq!(env.eval_int("exposure.getProvincePixel(0, 0)"), 7);
}

#[test]
fn the_copy_writes_the_main_image_only() {
    let _lock = vm_lock();
    let env = common::Env::new();
    env.link_plugin("layerExLongExposure.dll");
    make_layer(&env);
    env.run(
        "exposure.fillRect(0, 0, 4, 4, 0xFF0000FF); \
         exposure.initExposure(); \
         exposure.snapExposure();",
    );
    // The image the copy writes is marked modified, the way the writable
    // image buffer marks it (`upstream Main.cpp:120-123`).
    env.run("exposure.imageModified = false;");
    env.run("exposure.copyExposure(void);");
    assert_eq!(env.eval_int("exposure.imageModified"), 1);
    // The size of the image is unchanged.
    assert_eq!(env.eval_int("exposure.imageWidth"), 4);
    assert_eq!(env.eval_int("exposure.imageHeight"), 4);
}
