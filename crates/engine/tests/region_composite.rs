//! The composite redraws the region that changed and nothing else.
//!
//! The reference's cache keeps a recalculation region
//! (`reference/krkr2/cpp/core/visual/LayerIntf.h:963-965`), fills it from
//! the changed rectangles (`:6254`, `:6285`, `:6329`) and reconstructs alone
//! that region, leaving the cached bitmap untouched when the region is empty
//! (`:7734-7748`).

use std::time::Instant;

use engine::layer_ops::render_layer_subtree;
use engine::scene::{Rect, Scene};

/// The size of the composite plane: the logical screen.
const WIDTH: u32 = 1920;
/// The height of the composite plane.
const HEIGHT: u32 = 1080;
/// The number of contributors under the root layer.
const CONTRIBUTORS: usize = 16;

/// The sizes of the contributors, as the game's transition endpoints report
/// them: four full-size images and the rest smaller.
const SIZES: [(u32, u32); CONTRIBUTORS] = [
    (1920, 1080),
    (336, 192),
    (16, 16),
    (88, 97),
    (88, 97),
    (1920, 1080),
    (336, 192),
    (104, 98),
    (104, 98),
    (336, 192),
    (16, 16),
    (480, 320),
    (1920, 1080),
    (16, 16),
    (16, 16),
    (16, 16),
];

/// A scene whose root layer holds the composite plane and whose
/// contributors are the layers under it.
fn scene() -> (Scene, u32, Vec<u32>) {
    let mut scene = Scene::default();
    let window = scene.add_window("region", (WIDTH, HEIGHT));
    let root = scene.add_layer(window, None);
    {
        let layer = scene.layer_mut(root).expect("the root layer");
        layer.rect = Rect::new(0, 0, WIDTH, HEIGHT);
        layer.visible = true;
    }
    let mut contributors = Vec::new();
    for index in 0..CONTRIBUTORS {
        let (width, height) = SIZES[index % SIZES.len()];
        let id = scene.add_layer(window, Some(root));
        let bitmap = scene.add_bitmap(width, height, vec![7u8; (width * height * 4) as usize]);
        {
            let layer = scene.layer_mut(id).expect("the contributor");
            layer.rect = Rect::new(0, 0, width, height);
            layer.visible = true;
            layer.bitmap = Some(bitmap);
        }
        contributors.push(id);
    }
    // The root caches: the reference's shape is that a layer keeps a
    // recalculation region only while its caching is enabled
    // (`reference/krkr2/cpp/core/visual/LayerIntf.cpp:6318-6331`). The area a
    // layer reconstructs is its own region alone
    // (`reference/krkr2/cpp/core/visual/LayerIntf.cpp:7741-7743`).
    scene.inc_cache_enabled_count(root);
    // The setup marked the layers it touched; the tests start from a clean
    // tree.
    let ids: Vec<u32> = scene.layers.iter().map(|layer| layer.id).collect();
    for id in ids {
        let _ = scene.take_layer_content_region(id);
    }
    (scene, root, contributors)
}

/// The bytes of the composited content.
fn content_bytes(scene: &Scene, content: u32) -> Vec<u8> {
    scene
        .bitmap(content)
        .expect("the composited content")
        .rgba
        .clone()
}

/// A change to one contributor marks that contributor's own area, and the
/// ancestors hold it, so the composite of the root redraws the area alone.
#[test]
fn a_change_marks_the_area_of_the_layer_that_changed() {
    let (mut scene, root, contributors) = scene();
    let changed = contributors[3];
    let area = Rect::new(0, 0, 88, 97);
    // The contributor caches too, so it keeps a region of its own.
    scene.inc_cache_enabled_count(changed);

    assert_eq!(
        scene.layer_content_region(root),
        None,
        "a fresh tree has nothing to redraw"
    );
    scene
        .update_layer(changed, |layer| {
            layer.opacity = 0.5;
            true
        })
        .expect("the contributor is there");

    assert_eq!(
        scene.layer_content_region(changed),
        Some(area),
        "the layer marks its own rectangle"
    );
    assert_eq!(
        scene.layer_content_region(root),
        Some(area),
        "the ancestor holds the layer's contribution"
    );
}

/// The composite of the region writes the same pixels as the composite of
/// the whole plane: the region is where the work goes, not what the result
/// is.
#[test]
fn the_region_composite_writes_the_same_pixels_as_the_whole_plane() {
    let (mut scene, root, contributors) = scene();
    let changed = contributors[3];
    // The contributor caches too, so it keeps a region of its own.
    scene.inc_cache_enabled_count(changed);
    let content = render_layer_subtree(&mut scene, root).expect("the content");

    // A change to one contributor.
    scene
        .update_layer(changed, |layer| {
            layer.opacity = 0.5;
            true
        })
        .expect("the contributor is there");
    let region = scene
        .layer_content_region(root)
        .expect("the region to reconstruct");
    let region_bytes = region.area();

    // The composite of the region alone.
    let started = Instant::now();
    render_layer_subtree(&mut scene, root).expect("the content");
    let region_cost = started.elapsed().as_secs_f64() * 1000.0;
    let region_pixels = content_bytes(&scene, content);

    // The same composite over the whole plane: the shape the engine had
    // before the region.
    scene.invalidate_layer_content(root);
    assert_eq!(
        scene.layer_content_region(root),
        Some(Rect::new(0, 0, WIDTH, HEIGHT)),
        "the whole plane is the region of a full redraw"
    );
    let started = Instant::now();
    render_layer_subtree(&mut scene, root).expect("the content");
    let plane_cost = started.elapsed().as_secs_f64() * 1000.0;
    let plane_pixels = content_bytes(&scene, content);

    assert_eq!(
        region_pixels, plane_pixels,
        "the region composite writes the same pixels as the whole plane"
    );
    println!(
        "the plane {WIDTH}x{HEIGHT}: the region {region_bytes} px, the composite {region_cost:.3} ms, \
         the whole plane {plane_cost:.3} ms"
    );
}

/// A region that is empty leaves the cached bitmap untouched: the content
/// is not written at all, which is what makes a cached layer cheap.
#[test]
fn an_empty_region_leaves_the_content_untouched() {
    let (mut scene, root, _contributors) = scene();
    let content = render_layer_subtree(&mut scene, root).expect("the content");
    let before = content_bytes(&scene, content);
    // The composite wrote the content, so the bitmap is dirty.
    scene.set_bitmap_clean(content);

    // Nothing changed, so the region is empty.
    assert_eq!(scene.layer_content_region(root), None);
    let again = render_layer_subtree(&mut scene, root).expect("the content");
    assert_eq!(again, content, "the content is the same bitmap");
    assert!(
        !scene.bitmap(content).expect("the content").dirty,
        "an empty region writes no pixel"
    );
    assert_eq!(
        content_bytes(&scene, content),
        before,
        "an empty region leaves the content as it was"
    );
}

/// The region is where the win is: the composite of the region costs a
/// fraction of the composite of the whole plane, in proportion to the area.
///
/// The test is ignored by default: it measures the wall clock, and the gate
/// runs it only when the measurement is wanted.
#[test]
#[ignore]
fn the_region_costs_less_than_the_whole_plane() {
    let (mut scene, root, contributors) = scene();
    let changed = contributors[3];
    let _ = render_layer_subtree(&mut scene, root).expect("the content");
    let plane_area = u64::from(WIDTH) * u64::from(HEIGHT);

    let mut region_total = 0.0;
    let mut plane_total = 0.0;
    let mut region_area = 0u64;
    for _ in 0..10 {
        scene
            .update_layer(changed, |layer| {
                layer.opacity = if layer.opacity == 0.5 { 0.25 } else { 0.5 };
                true
            })
            .expect("the contributor is there");
        let region = scene
            .layer_content_region(root)
            .expect("the region to reconstruct");
        region_area = region.area();
        let started = Instant::now();
        render_layer_subtree(&mut scene, root).expect("the content");
        region_total += started.elapsed().as_secs_f64() * 1000.0;

        scene.invalidate_layer_content(root);
        let started = Instant::now();
        render_layer_subtree(&mut scene, root).expect("the content");
        plane_total += started.elapsed().as_secs_f64() * 1000.0;
    }
    let region_total = region_total / 10.0;
    let plane_total = plane_total / 10.0;
    println!(
        "the plane {WIDTH}x{HEIGHT} ({plane_area} px): the region {region_area} px \
         ({:.2} % of the plane), the composite {region_total:.3} ms, the whole plane \
         {plane_total:.3} ms, the factor {:.1}",
        100.0 * region_area as f64 / plane_area as f64,
        plane_total / region_total.max(f64::EPSILON),
    );
    assert!(
        region_total < plane_total,
        "the region composite is cheaper than the whole plane"
    );
}

/// The scene transition: a full-screen crossfade between two whole subtree
/// composites at the logical screen.
///
/// The transition's frame is the crossfade over the whole screen; the two
/// endpoints are composited once each, and the ticks after that are cache
/// hits — the transition's tick changes no source, so it appends to no
/// region (`reference/krkr2/cpp/core/visual/LayerIntf.cpp:6322-6327`).
///
/// The test is ignored by default: it measures the wall clock, and the gate
/// runs it only when the measurement is wanted.
#[test]
#[ignore]
fn a_scene_transition_costs_the_crossfade_and_two_composites() {
    use engine::trans::Trans;

    let (mut scene, root, _contributors) = scene();
    let content = render_layer_subtree(&mut scene, root).expect("the content");

    // The first composite: the screen's subtree changed everywhere, so the
    // whole plane is what has to be drawn.
    let mut first = 0.0;
    let mut hits = 0.0;
    for _ in 0..10 {
        scene.invalidate_layer_content(root);
        let started = Instant::now();
        render_layer_subtree(&mut scene, root).expect("the content");
        first += started.elapsed().as_secs_f64() * 1000.0;
        // The tick after it: nothing changed, so the content is current.
        let started = Instant::now();
        render_layer_subtree(&mut scene, root).expect("the content");
        hits += started.elapsed().as_secs_f64() * 1000.0;
    }
    let first = first / 10.0;
    let hits = hits / 10.0;

    // The crossfade over the whole screen: the transition's frame.
    let width = WIDTH as usize;
    let height = HEIGHT as usize;
    let pixels = width * height * 4;
    let target = scene.add_bitmap(WIDTH, HEIGHT, vec![0u8; pixels]);
    let source = scene.add_bitmap(WIDTH, HEIGHT, vec![3u8; pixels]);
    let other = scene.add_bitmap(WIDTH, HEIGHT, vec![7u8; pixels]);
    let mut trans = Trans::cross_fade(1000.0).expect("the cross-fade");
    // The middle of the fade, where the frame mixes the two sources.
    trans.seek_ms(500.0);
    let mut blend = 0.0;
    for _ in 0..10 {
        let started = Instant::now();
        let region = Trans::full_region(scene.bitmap(target).expect("the target"));
        let (target, source, other) = scene
            .bitmap_mut_with_refs(target, source, other)
            .expect("the three bitmaps");
        trans
            .apply(target, source, other, region)
            .expect("the blend");
        blend += started.elapsed().as_secs_f64() * 1000.0;
    }
    let blend = blend / 10.0;

    let plane = u64::from(WIDTH) * u64::from(HEIGHT);
    println!(
        "the scene transition at {WIDTH}x{HEIGHT} ({plane} px): the endpoint composite \
         {first:.3} ms, the tick's cache hit {hits:.3} ms, the crossfade {blend:.3} ms \
         ({:.2} ns/px), a 60-frame transition {:.1} ms",
        blend * 1e6 / plane as f64,
        60.0 * blend,
    );
    let _ = content;
}

/// The composite's cost as the region grows: the curve the reference's
/// precision rides on.
///
/// A change that names the rectangle it touched is redrawn over that
/// rectangle alone; a change that names nothing is redrawn over the whole
/// plane. The sweep is what the two cost.
///
/// The test is ignored by default: it measures the wall clock, and the gate
/// runs it only when the measurement is wanted.
#[test]
#[ignore]
fn the_composite_costs_what_the_region_costs() {
    let (mut scene, root, contributors) = scene();
    let changed = contributors[3];
    let plane_area = u64::from(WIDTH) * u64::from(HEIGHT);

    println!("the plane {WIDTH}x{HEIGHT} ({plane_area} px):");
    for side in [16u32, 64, 256, 512, 960, 1080] {
        scene
            .update_layer(changed, |layer| {
                layer.rect = Rect::new(0, 0, side, side);
                true
            })
            .expect("the contributor is there");
        let region = scene
            .layer_content_region(root)
            .expect("the region to reconstruct");
        let area = region.area();
        let mut total = 0.0;
        for _ in 0..10 {
            scene
                .update_layer(changed, |layer| {
                    layer.opacity = if layer.opacity == 0.5 { 0.25 } else { 0.5 };
                    true
                })
                .expect("the contributor is there");
            let started = Instant::now();
            render_layer_subtree(&mut scene, root).expect("the content");
            total += started.elapsed().as_secs_f64() * 1000.0;
        }
        println!(
            "  a {side}x{side} region ({area} px, {:.1} % of the plane): the composite {:.3} ms",
            100.0 * area as f64 / plane_area as f64,
            total / 10.0,
        );
    }
}

/// The transition's display does not take the destination's content.
///
/// The display takes the layer's cache slot, and the slot may hold the
/// layer's composited content: the script's cached-layer pass puts it there
/// until the layer is in a transition
/// (`crates/compat/src/natives/visual/layer/mod.rs`). The content survives:
/// the release of the slot's bitmap is refused while a layer still holds it
/// (`Scene::release_bitmap`), so the composite keeps the same content bitmap
/// and no second one is allocated.
///
/// The reference keeps the same two bitmaps apart: the layer's `CacheBitmap`
/// and the transition's own frame
/// (`reference/krkr2/cpp/core/visual/LayerIntf.cpp:6431-6435`).
#[test]
fn the_transitions_display_keeps_the_destinations_content() {
    let (mut scene, dest, _contributors) = scene();
    let content = render_layer_subtree(&mut scene, dest).expect("the content");

    // The display takes the cache slot, as the transition's frame step does.
    let display = engine::layer_ops::ensure_transition_display(&mut scene, dest, WIDTH, HEIGHT)
        .expect("the display");
    assert_ne!(display, content, "the display is a bitmap of its own");
    assert!(
        scene.bitmap(content).is_some(),
        "the content survives the display that took the slot"
    );

    // The next composite reuses it: no second whole-screen content.
    let again = render_layer_subtree(&mut scene, dest).expect("the content");
    assert_eq!(again, content, "the composite keeps the content bitmap");
    assert!(
        scene.bitmap(content).is_some(),
        "the content is still the scene's bitmap"
    );
}

/// The transition's teardown releases the transition's own cache consumer,
/// not the layer's content.
///
/// The content's owner is the layer's cache count: the content is freed with
/// the last consumer (`Scene::dec_cache_enabled_count`). A layer that caches
/// holds a consumer of its own, so the transition's count pair
/// (`reference/krkr2/cpp/core/visual/LayerIntf.cpp:7961-7964`) moves the
/// count `1 -> 2 -> 1` and the content survives the teardown. The next
/// composite then has nothing to reconstruct: the whole screen is not
/// composited again.
#[test]
fn the_transitions_teardown_keeps_the_destinations_content() {
    let (mut scene, dest, _contributors) = scene();
    let content = render_layer_subtree(&mut scene, dest).expect("the content");

    // The transition: the endpoint's cache is enabled and the frame step
    // takes the slot for the display.
    scene.inc_cache_enabled_count(dest);
    let _ = engine::layer_ops::ensure_transition_display(&mut scene, dest, WIDTH, HEIGHT)
        .expect("the display");

    // The teardown releases the transition's consumer; the layer's own
    // consumer still holds the content.
    scene.dec_cache_enabled_count(dest);
    assert!(
        scene.bitmap(content).is_some(),
        "the teardown releases the transition's consumer, not the content"
    );

    // The next composite keeps the content and writes no pixel: the region
    // is empty, so the content is current.
    scene.set_bitmap_clean(content);
    let again = render_layer_subtree(&mut scene, dest).expect("the content");
    assert_eq!(again, content, "the composite keeps the content bitmap");
    assert!(
        !scene.bitmap(content).unwrap().dirty,
        "an empty region writes no pixel"
    );
}
