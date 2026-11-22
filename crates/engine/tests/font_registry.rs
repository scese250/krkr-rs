//! Integration tests for the L2 font registry, the per-character face
//! resolution and the measure-versus-draw split.
//!
//! The faces are synthetic (`testkit::font_fixtures`), so the tests do not
//! depend on the host's fonts: the narrow face carries Latin glyphs only and
//! the wide face carries one CJK glyph the narrow face does not have.
//!
//! Reference: the font map is keyed by the family names of the enumerated
//! faces and its oldest entry is the default font
//! (`reference/cpp/core/visual/FontImpl.cpp:242-245`); the rasterizer asks
//! the requested face for a character, then the default font, and draws the
//! face's default character when neither carries it
//! (`reference/cpp/core/visual/FreeTypeFontRasterizer.cpp:180-194`); a
//! measurement follows the requested face
//! (`reference/cpp/core/visual/LayerIntf.cpp:11702-11713`).

use std::path::PathBuf;

use engine::font::FontId;
use engine::text::{TextEngine, TextError, TextStyle};
use testkit::font_fixtures::{
    NARROW_NAMES, WIDE_NAMES, WIDE_ONLY_CODEPOINT, narrow_face, wide_face,
};

/// The pixel size the measurements use.
const SIZE_PX: f32 = 32.0;

/// The narrow face's 'A' advance: 400 design units of a 1000-unit em, so a
/// third of the em, rounded to whole pixels.
const NARROW_A_ADVANCE: f32 = 13.0;

/// The wide face's CJK advance: 900 design units of a 1000-unit em, rounded to
/// whole pixels.
const WIDE_CJK_ADVANCE: f32 = 29.0;

/// A character no face carries: a private-use code point.
const UNCARRYABLE: char = '\u{10FFFD}';

/// The style the measurements use.
fn style() -> TextStyle {
    TextStyle::new(SIZE_PX)
}

/// An engine with the wide face registered first, so the wide face is the
/// default face the per-character fallback reaches for.
fn engine_with_wide_default() -> (TextEngine, FontId, FontId) {
    let mut engine = TextEngine::new();
    let wide = engine
        .add_font(&wide_face(), 0)
        .expect("register the wide face");
    let narrow = engine
        .add_font(&narrow_face(), 0)
        .expect("register the narrow face");
    (engine, wide, narrow)
}

/// The default face is the oldest registered entry: the face registered
/// first stays the default, and a later registration does not move it.
///
/// Reference: the font map's oldest entry is the default font
/// (`reference/cpp/core/visual/FontImpl.cpp:242-245`).
#[test]
fn the_default_face_is_the_oldest_registered_entry() {
    let mut engine = TextEngine::new();
    let narrow = engine
        .add_font(&narrow_face(), 0)
        .expect("register the narrow face");
    assert_eq!(
        engine.default_face(),
        Some(narrow),
        "the first registered face is the default face"
    );
    assert_eq!(engine.registry().default_name(), Some(NARROW_NAMES[0]));
    let wide = engine
        .add_font(&wide_face(), 0)
        .expect("register the wide face");
    assert_eq!(
        engine.default_face(),
        Some(narrow),
        "a later registration must not move the default face"
    );
    assert_ne!(engine.default_face(), Some(wide));
}

/// The registered-name list is newest first and leaves the oldest name out.
///
/// Reference: the font list enumerates the map and stops at its oldest entry,
/// so the default face's name is not in the list and a map that holds a single
/// name reports an empty list
/// (`reference/cpp/core/visual/FontImpl.cpp:242-245`).
#[test]
fn the_registered_name_list_is_newest_first_without_the_oldest_name() {
    let mut engine = TextEngine::new();
    engine
        .add_font(&narrow_face(), 0)
        .expect("register the narrow face");
    assert_eq!(
        engine.registry().list_names(),
        vec![NARROW_NAMES[1].to_string()],
        "the oldest name is the default face's name and stays out of the list"
    );
    engine
        .add_font(&wide_face(), 0)
        .expect("register the wide face");
    assert_eq!(
        engine.registry().list_names(),
        vec![
            WIDE_NAMES[1].to_string(),
            WIDE_NAMES[0].to_string(),
            NARROW_NAMES[1].to_string(),
        ],
        "the list is newest first and leaves the oldest name out"
    );
}

/// A face answers to each of its names, in either case.
///
/// Reference: the font map is keyed by every family name the enumerated face
/// carries (`reference/cpp/core/visual/FontImpl.cpp:82-124`) and a lookup
/// matches the requested name against that map
/// (`reference/cpp/core/visual/FontImpl.cpp:277-291`).
#[test]
fn a_face_resolves_under_each_of_its_names() {
    let mut engine = TextEngine::new();
    let narrow = engine
        .add_font(&narrow_face(), 0)
        .expect("register the narrow face");
    assert_eq!(engine.font_id(NARROW_NAMES[0]), Some(narrow));
    assert_eq!(engine.font_id(NARROW_NAMES[1]), Some(narrow));
    assert_eq!(
        engine.font_id(&NARROW_NAMES[0].to_lowercase()),
        Some(narrow),
        "the lookup is case-insensitive"
    );
    assert_eq!(engine.font_id("no-such-family"), None);
}

/// The requested face draws the characters it carries: the fallback never
/// displaces a face the character belongs to.
///
/// Reference: the rasterizer asks the requested face first
/// (`reference/cpp/core/visual/FreeTypeFontRasterizer.cpp:180-181`).
#[test]
fn the_requested_face_draws_the_characters_it_carries() {
    let (mut engine, wide, narrow) = engine_with_wide_default();
    assert!(
        engine.has_glyph(narrow, 'A'),
        "the narrow face must carry the Latin glyph"
    );
    assert!(
        engine.has_glyph(wide, 'A'),
        "the default face must carry the Latin glyph too"
    );
    assert_eq!(
        engine.provider_for(narrow, 'A').expect("the provider"),
        narrow,
        "a character the requested face carries is drawn by it"
    );
    assert_eq!(
        engine.provider_for(wide, 'A').expect("the provider"),
        wide,
        "the default face draws the character it carries"
    );
}

/// The default face draws a character the requested face does not carry.
///
/// Reference: the rasterizer falls back to the default font for a character
/// the requested face does not carry
/// (`reference/cpp/core/visual/FreeTypeFontRasterizer.cpp:182-186`).
#[test]
fn the_default_face_draws_the_characters_the_requested_face_lacks() {
    let (mut engine, wide, narrow) = engine_with_wide_default();
    assert_eq!(
        engine.default_face(),
        Some(wide),
        "the default face is the oldest registered entry"
    );
    assert!(
        !engine.has_glyph(narrow, WIDE_ONLY_CODEPOINT),
        "the requested face must not carry the CJK glyph"
    );
    assert!(
        engine.has_glyph(wide, WIDE_ONLY_CODEPOINT),
        "the default face must carry the CJK glyph"
    );
    assert_eq!(
        engine
            .provider_for(narrow, WIDE_ONLY_CODEPOINT)
            .expect("the provider"),
        wide,
        "a character the requested face lacks is drawn by the default face"
    );
}

/// The fallback is per character: a character the requested face does not map
/// is drawn by the default face, and a character it does map is drawn by
/// itself.
///
/// Reference: the rasterizer asks the requested face first and falls back to
/// the default face for a character the requested face does not carry
/// (`reference/cpp/core/visual/FreeTypeFontRasterizer.cpp:180-186`).
#[test]
fn the_fallback_is_per_character() {
    let (mut engine, wide, narrow) = engine_with_wide_default();
    assert!(
        !engine.has_glyph(narrow, WIDE_ONLY_CODEPOINT),
        "the narrow face must not carry the CJK glyph"
    );
    assert!(engine.has_glyph(wide, WIDE_ONLY_CODEPOINT));
    assert_eq!(
        engine.provider_for(narrow, 'A').expect("the provider"),
        narrow,
        "a character the requested face carries is drawn by it"
    );
    assert_eq!(
        engine
            .provider_for(narrow, WIDE_ONLY_CODEPOINT)
            .expect("the provider"),
        wide,
        "a character the requested face lacks is drawn by the default face"
    );
    assert!(
        matches!(
            engine.provider_for(wide, WIDE_ONLY_CODEPOINT),
            Ok(id) if id == wide
        ),
        "the default face draws the character it carries"
    );
}

/// A character no face carries is drawn with the face's own default
/// character: the run continues instead of raising.
///
/// Reference: the rasterizer draws the face's default character and then its
/// first character when the requested face and the default font carry
/// neither, and raises only when the face has no glyph at all
/// (`reference/cpp/core/visual/FreeTypeFontRasterizer.cpp:188-194`); the
/// Windows rasterizer draws an empty glyph and continues
/// (`reference/cpp/core/visual/impl/GDIFontRasterizer.cpp:170,218-220`).
#[test]
fn a_character_no_face_carries_is_drawn_with_the_substitute() {
    let (mut engine, _wide, narrow) = engine_with_wide_default();
    assert!(
        !engine.has_glyph(narrow, UNCARRYABLE),
        "no face may carry the private-use character"
    );
    assert_eq!(
        engine
            .provider_for(narrow, UNCARRYABLE)
            .expect("the substitute is not an error"),
        narrow,
        "the requested face draws its own default character"
    );
    let drawn = engine
        .rasterize_glyph(narrow, UNCARRYABLE, &style())
        .expect("the substitute is drawn");
    assert!(
        !drawn.is_empty(),
        "the face's default character is drawn in place of the missing one"
    );
    assert_eq!(
        drawn.advance, 16.0,
        "the substitute advances like the face's default character"
    );
}

/// A face this engine does not know has no provider: the resolution reports
/// it instead of naming a face that cannot draw the character.
///
/// Reference: the rasterizer raises when the requested face has no glyph to
/// offer (`reference/cpp/core/visual/FreeTypeFontRasterizer.cpp:180-194`).
#[test]
fn an_unregistered_face_is_reported() {
    let mut engine = TextEngine::new();
    let mut other = TextEngine::new();
    let foreign = other
        .add_font(&narrow_face(), 0)
        .expect("register the face in another engine");
    let Err(error) = engine.provider_for(foreign, 'A') else {
        panic!("an unregistered face must be reported");
    };
    assert!(
        matches!(error, TextError::UnknownFont(_)),
        "an unregistered face must be reported: {error:?}"
    );
}

/// The measurement follows the requested face, the drawing follows the face
/// that maps the character.
///
/// Reference: the measurement is the requested face's extent, and a character
/// the requested face does not carry measures the face's height box
/// (`reference/cpp/core/visual/LayerIntf.cpp:11702-11713`); the drawn advance
/// is the drawn glyph's own advance
/// (`reference/cpp/core/visual/impl/LayerBitmapImpl.cpp:1367`).
#[test]
fn the_measurement_follows_the_requested_face_and_the_drawing_the_provider() {
    let (mut engine, wide, narrow) = engine_with_wide_default();
    let style = style();
    assert_eq!(
        engine
            .measure_code_unit(narrow, 'A', &style)
            .expect("measure"),
        NARROW_A_ADVANCE,
        "a character the requested face carries measures the face's advance"
    );
    assert_eq!(
        engine
            .measure_code_unit(narrow, WIDE_ONLY_CODEPOINT, &style)
            .expect("measure"),
        SIZE_PX,
        "a character the requested face lacks measures the face's em box"
    );
    let drawn = engine
        .rasterize_glyph(wide, WIDE_ONLY_CODEPOINT, &style)
        .expect("the default face draws the character");
    assert_eq!(
        drawn.advance, WIDE_CJK_ADVANCE,
        "the drawn advance is the provider's advance"
    );
    assert_ne!(
        drawn.advance,
        engine
            .measure_code_unit(narrow, WIDE_ONLY_CODEPOINT, &style)
            .expect("measure the requested face"),
        "the measured and the drawn advance differ for a fallback character"
    );
}

/// A shaped run names the face that draws each of its characters, so a
/// character the requested face lacks is drawn by the default face.
#[test]
fn a_shaped_run_names_the_face_that_draws_each_character() {
    let (mut engine, wide, narrow) = engine_with_wide_default();
    let style = style();
    let layout = engine
        .layout(narrow, "A\u{4E00}", &style, None)
        .expect("shape the run");
    assert_eq!(layout.glyphs.len(), 2);
    assert_eq!(layout.glyphs[0].codepoint, 'A');
    assert_eq!(layout.glyphs[0].font, narrow);
    assert_eq!(layout.glyphs[1].codepoint, WIDE_ONLY_CODEPOINT);
    assert_eq!(
        layout.glyphs[1].font, wide,
        "the fallback character names the face that draws it"
    );
}

/// A character the requested face and the default face do not carry is drawn
/// by a host face that carries it: the host's faces are searched for the
/// character itself.
///
/// Reference: the rasterizer falls back to the default font for a character
/// the requested face does not carry
/// (`reference/cpp/core/visual/FreeTypeFontRasterizer.cpp:182-186`); the
/// default font is the host's own font, which the enumeration provides
/// (`reference/cpp/core/visual/FontImpl.cpp:178-190`).
#[test]
fn a_host_face_carries_the_character_the_requested_face_lacks() {
    let Some(dir) = host_font_dir() else {
        return;
    };
    let mut engine = TextEngine::new();
    engine.set_system_font_dirs(vec![dir.clone()]);
    engine.set_host_faces(vec![NARROW_NAMES[0].to_string()]);
    let default = engine
        .default_face()
        .expect("the host's default face is registered");
    assert!(
        !engine.has_glyph(default, WIDE_ONLY_CODEPOINT),
        "the host's default face must not carry the CJK glyph"
    );
    let provider = engine
        .provider_for(default, WIDE_ONLY_CODEPOINT)
        .expect("the host's faces carry the character");
    assert_ne!(
        provider, default,
        "the host's own face must not be asked to carry the character"
    );
    assert!(
        engine.has_glyph(provider, WIDE_ONLY_CODEPOINT),
        "the provider must carry the character"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The search over the host's faces is memoized per character: a repeated
/// request for the same character reuses the face the search found instead of
/// reading the host's font files again.
#[test]
fn the_host_face_search_is_memoized_per_character() {
    let Some(dir) = host_font_dir() else {
        return;
    };
    let mut engine = TextEngine::new();
    engine.set_system_font_dirs(vec![dir.clone()]);
    engine.set_host_faces(vec![NARROW_NAMES[0].to_string()]);
    let default = engine
        .default_face()
        .expect("the host's default face is registered");
    let first = engine
        .provider_for(default, WIDE_ONLY_CODEPOINT)
        .expect("the host's faces carry the character");
    let registered = engine.registry().len();
    let second = engine
        .provider_for(default, WIDE_ONLY_CODEPOINT)
        .expect("the host's faces carry the character");
    assert_eq!(first, second, "the memoized face must be the one found");
    assert_eq!(
        engine.registry().len(),
        registered,
        "a repeated request must not register the face a second time"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A directory holding the synthetic faces, for the host's own font
/// directories.
///
/// `None` when the directory cannot be written.
fn host_font_dir() -> Option<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir =
        std::env::temp_dir().join(format!("krkr-font-registry-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(dir.join("narrow.ttf"), narrow_face()).ok()?;
    std::fs::write(dir.join("wide.ttf"), wide_face()).ok()?;
    Some(dir)
}
