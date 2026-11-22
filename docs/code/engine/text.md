# `engine::text` — shaping, coverage, atlas and text projection

Source: `crates/engine/src/text.rs`, `crates/engine/src/font.rs`

The module delegates the hard parts:

* **Layout / shaping / bidi / line-breaking / font fallback** go to
  `cosmic-text` (harfbuzz shaping, `fontdb` discovery and fallback,
  `unicode-bidi`, `unicode-linebreak`).
* **Rasterization** is the L1 `render::FontSystem` contract — L1 owns the
  font and the coverage bitmap; **L2 owns the atlas** that packs those
  bitmaps.
* **GPU projection** is the L1 quad pipeline: `project_text` turns positioned
  glyphs into per-glyph textured quads against an L2-owned `GlyphAtlas`.

Script-visible text is drawn by compositing per-character coverage into the
layer's `BitmapState` (`rasterize_text_coverage` + `compose_text_into`), so
`getPixel` and every other readback observe the same pixels the reference
produces. The `GlyphAtlas` path rasterizes the same coverage once, packs it
into a texture, and `project_text` emits textured quads; it needs a live
`RenderPipeline`.

---

## 1. Responsibility

* Own the registered faces and the shaping system.
* Lay a string out and return positioned glyphs with their advances.
* Rasterize glyphs into reusable coverage masks, and blur them for the
  shadow pass.
* Pack glyphs into an atlas texture and project them into a `RenderScene`.
* Composite a text run into a bitmap.

---

## 2. State it owns

Everything is **explicit caller-owned state**; there are no globals, no
statics and no hidden caches.

| value | state |
|---|---|
| `TextEngine` | the L1 `FontSystem`, the `cosmic-text` shaping system, the font registry, the host font database the background scan builds, the memoized shaped layouts, the memoized per-character advances, the memoized missing system families and the memoized host faces that carry a character |
| `FontRegistry` | the registered faces, the names they answer to, the registered-name order and the default face |
| `GlyphAtlas` | the atlas texture, its CPU mirror, the shelf packer, the glyph entry map and the pending GPU uploads |
| `TextCoverageCache` | the rasterized character masks and their blurred shadows |

The six L2 memos — the shaped layouts, the per-character advances, the
missing system families, the host faces that carry a character, the coverage
masks and the atlas entries — share the bound `MAX_TEXT_CACHE_ENTRIES` (1300)
and evict least-recently-used.

---

## 3. Public API

### `TextEngine`

```rust
impl TextEngine {
    pub fn new() -> Self;
    pub fn set_system_font_dirs(&mut self, dirs: Vec<PathBuf>);
    pub fn set_host_faces(&mut self, default_families: Vec<String>);
    pub fn add_font(&mut self, bytes: &[u8], index: u32) -> Result<FontId, TextError>;
    pub fn font_id(&self, family: &str) -> Option<FontId>;
    pub fn alias(&mut self, name: &str, id: FontId);
    pub fn registry(&self) -> &FontRegistry;
    pub fn default_face(&mut self) -> Option<FontId>;
    pub fn default_face_name(&mut self) -> Option<&str>;
    pub fn registered_face_names(&mut self) -> Vec<String>;
    pub fn load_system_font(&mut self, family: &str) -> Result<FontId, TextError>;
    pub fn install_host_face(&mut self) -> Result<Option<FontId>, TextError>;
    pub fn face(&self, id: FontId) -> Option<&Font>;
    pub fn family_name(&self, id: FontId) -> Option<&str>;
    pub fn has_glyph(&self, id: FontId, codepoint: char) -> bool;
    pub fn provider_for(&mut self, id: FontId, codepoint: char) -> Result<FontId, TextError>;
    pub fn measure_code_unit(&mut self, id: FontId, codepoint: char, style: &TextStyle)
        -> Result<f32, TextError>;
    pub fn validate_text(&self, id: FontId, style: &TextStyle) -> Result<(), TextError>;
    pub fn measure_text(&mut self, id: FontId, text: &str, style: &TextStyle)
        -> Result<f64, TextError>;
    pub fn font_metrics(&self, id: FontId, size_px: f32) -> Result<FontMetrics, TextError>;
    pub fn layout(&mut self, font: FontId, text: &str, style: &TextStyle,
                  wrap_width: Option<f32>) -> Result<TextLayout, TextError>;
    pub fn cached_layouts(&self) -> usize;
    pub fn system_database_builds(&self) -> usize;
    pub fn rasterize_glyph(&self, font: FontId, codepoint: char,
                           style: &TextStyle) -> Result<GlyphBitmap, TextError>;
    pub fn glyph_advance(&mut self, font: FontId, codepoint: char, style: &TextStyle)
        -> Result<f32, TextError>;
    pub fn glyph_draw_rect(&self, font: FontId, codepoint: char, style: &TextStyle)
        -> Result<Option<(f32, f32, f32, f32)>, TextError>;
}
```

### The font registry

```rust
impl FontRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, face: FontFace) -> FontId;
    pub fn alias(&mut self, name: &str, id: FontId);
    pub fn by_name(&self, name: &str) -> Option<FontId>;
    pub fn face(&self, id: FontId) -> Option<&FontFace>;
    pub fn names(&self) -> &[String];
    pub fn list_names(&self) -> Vec<String>;
    pub fn default_name(&self) -> Option<&str>;
    pub fn default_face(&self) -> Option<FontId>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

The registry is the **one** place that knows which faces exist:

* A face is registered under **every** family name its name table carries
  (the typographic family first, then the plain family, in every language the
  face declares), so a lookup by any of them finds it. The names are matched
  case-insensitively.
* The **default face** is the **oldest registered face** — the first face the
  registry ever got. A later registration does not move it, and neither does
  a name that is registered again: the name order moves a name that is
  registered twice to the front of the list, so it cannot name the oldest
  face. The engine keeps the id of the face it registered first.
  The default face supplies the metrics of a face name that resolves to
  nothing, so the host's own default font is registered first: the preferred
  family when the host provides it, and one of the host's own faces when it
  provides none of them.
* The **default face** is also the face the per-character fallback reaches
  for when the requested face does not carry a character.
* The **registered-name list** is newest first and leaves the oldest name
  out, so a registry that holds a single name reports an empty list. The
  reference's font list stops at the map's oldest entry for the same reason
  (`reference/cpp/core/visual/FontImpl.cpp:242-245`).
* An **alias** is a name a caller asked for (a storage path, a
  comma-separated candidate list). It resolves the face but stays out of the
  registered-name list.

### The per-character fallback

`provider_for` answers the face that draws one character, and it answers it
in this order:

1. **the requested face**, when it maps the character;
2. **the default face**, when the default face maps it. The default face is
   the engine's default font, so on a host that does not carry the game's
   own font it is the host's font the engine installed
   (`reference/cpp/core/visual/FreeTypeFontRasterizer.cpp:182-186`);
3. **a host face that maps this character**, found by searching the host's
   installed faces for the character itself. The search stops at the first
   face that carries it;
4. **the requested face's own default character** — the glyph the face draws
   for a character it does not map
   (`reference/cpp/core/visual/FreeTypeFontRasterizer.cpp:188-194`).

The chain never raises for a character no face carries: the face's default
character is drawn in its place and the run continues
(`reference/cpp/core/visual/impl/GDIFontRasterizer.cpp:170,218-220`). Only a
face that carries no glyph at all has nothing left to draw, and that is the
one case the resolution reports as `TextError::MissingGlyph`.

`layout` records the resolved face in every `LaidOutGlyph`, so the drawing
path draws each character with the face that carries it. A resolution that
cannot name a face is an error the caller sees: the layout propagates it
instead of dropping it.

The **measurement** and the **drawing** follow different rules on purpose:

* `measure_text` is the sum of the per-character advances of the **requested**
  face, and it shapes nothing: the reference measures a run with the font's
  metrics, one character at a time
  (`reference/cpp/core/visual/LayerIntf.cpp:11702-11717`), so a width never
  enters the layout cache.
* `measure_code_unit` measures the **requested** face: a character the face
  maps measures the rasterizer's advance for it, and a character the face
  does not map measures the face's em box (the requested size in pixels). The
  face that would draw the character never contributes to a measurement
  (`reference/cpp/core/visual/LayerIntf.cpp:11702-11713`).
* The drawn advance is the drawn glyph's own advance, so it is the provider's
  (`reference/cpp/core/visual/impl/LayerBitmapImpl.cpp:1367`).

`add_font` loads the same bytes into the L1 rasterizer **and** the shaping
database, so layout and rasterization agree on the face, and registers the
face under every family name its name table carries.

`load_system_font` resolves a family against the host's installed fonts
(case-insensitively, including the localized family names in the name table);
only the resolved face enters the shaping database. A family the host does
not provide is memoized, so a repeated query reports the same
`TextError::FontNotFound` without scanning the host font database again. The
memo holds misses only: a family the host does provide resolves on the first
query, a face registered afterwards is found by `font_id` before the memo is
consulted, and the memo is dropped when `set_system_font_dirs` rebuilds the
font database.

`set_host_faces` selects the host's font role: the families the host
prefers for its own default font. The preferred families are a **preference**,
not a requirement: a host that names its fonts differently still has a
default face. The **default face is resolved on the first use of the text
service**, not when the role is selected, so a game that asks about no font
never registers a host face. The resolution is guarded by a flag that lives
with the state it guards, the way the reference's font system keeps its
initialization flag beside the font names it fills; the work happens on
whichever thread asks first, and it happens once.

Selecting the role also starts the **host font scan**: the host's font
directories are read on a thread of its own, so the request that needs a host
face does not read them on the thread that asks. The scan publishes the
database it built, and the first use takes it — waiting for the scan if it is
still running. The scan's report — the number of faces it found and the number
of directories it read — is printed where the faces are used, so the log
names what the scan found at the point the text service takes it.

**The scan is not on the first text operation's critical path.** The scan
reads the host's whole font directory — every font file, with every face of
every collection — and it costs hundreds of milliseconds of CPU on its own
thread (measured on the reference host: 233 ms in a debug build at a load of
80, 703 ms at a load of 107). The scan starts when the host selects its
role, so it runs while the game starts up; the first text use waits for it
only when it is still running, and the measured wait at the first text use
was **0.7 ms** (the scan had finished 4.4 s earlier).

The enumeration is the shape that is left open: the reference enumerates the
host's fonts once, on the first font query
(`reference/cpp/core/visual/FontImpl.cpp:178-190`), and its enumeration
touches a handful of files, while the port enumerates the host's whole
directory — and the host's directory list overlaps the locations the font
database scans on its own, so a file can be read twice. Making the
enumeration cheap needs the host's font *files* rather than its font
*directories* (a per-family lookup the composition root would supply), which
the text service's current protocol does not carry.

A scan reads the directories it was started with, so a new directory list
starts a new scan. A text service whose host selected no role has no scan:
the request that needs a host face reads the font directories itself, on its
own thread, and the host's own faces resolve. A scan that failed does not take
the host's faces away: the directories are read again on the thread that
asks, the way a text service without a scan reads them, and the failure is
reported. A read that fails again is reported as `TextError::HostFontScan` —
the request is told that the host's faces are unknown instead of being given
a face the host does not have.

The resolution registers the host's default face first, so it is the oldest
face and therefore the engine's default face: the face a face name that
resolves to nothing takes its metrics from. The default face is the first of
the preferred families the host provides; a host that provides none of them
still has one, because the engine registers one of the host's own faces
instead of losing the faces the scan found, and a face the rasterizer cannot
load is passed over so that the next face the host carries can serve. A host
with no usable font at all has no default face: the absence is reported, and
the operation that needs a face raises. The host's own faces stay reachable
under their own names, and a preferred family the host does not provide is
still reported as absent when it is asked for by name. A registry that
already holds a face keeps it.

`install_host_face` registers one of the host's own faces: the face the
engine falls back to when the host names its fonts differently than the
families the engine asked for. A face the rasterizer cannot load is passed
over, so a host that carries one unusable face still gets a face from the
next face it carries.

The per-character search over the host's faces is **memoized per
character**: the face it found for a character is remembered, so a repeated
request for the same character does not read the host's font files again.
The memo lives in the text engine's own state, is dropped with the font
database it describes, and is bounded by the same entry limit as the other
L2 memos.

A host whose faces cannot be read yields no face in that search, and the
failure is recorded: the resolution continues with the faces that are
registered and the character is drawn with the face's default character. The
typed error belongs to the path where a *named* family is asked for and the
host's faces are unknown (`load_system_font`).

### The pre-rendered font's file

A font mapped with `mapPrerenderedFont` is a **file image**, and the image is
read when the mapping is created: the reference's constructor reads the whole
file and holds it (`reference/cpp/core/visual/PrerenderedFont.cpp:9-76`), so
the glyph descriptors are read from the image and the storage is not read
again (`:60-62`). The header is validated at the map and the glyph body is
parsed on the first use, because the index bounds are not checked at the map.

The consequence for the frame times: the first text operation of a run does
not wait for the file. A 6.6 MB pre-rendered font read through the storage's
read pool costs about 30 ms in a debug build (measured on the reference
host) and it is a *wait* on a worker, so it grows with the machine's load;
reading it at the map moves it to the mapping, where the reference reads it.

### The glyph atlas

```rust
impl GlyphAtlas {
    pub fn new(pipeline: &RenderPipeline, size: u32) -> Result<Self, TextError>;
    pub fn clear(&mut self);
    pub fn texture(&self) -> &Texture;
    pub fn pixels(&self) -> &[u8];
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn pending_len(&self) -> usize;
    pub fn glyph(&mut self, engine: &TextEngine, font: FontId, codepoint: char,
                 style: &TextStyle) -> Result<AtlasGlyph, TextError>;
    pub fn flush(&mut self, pipeline: &RenderPipeline, state: &mut RenderState)
        -> Result<(), TextError>;
    pub fn rasterize_layout(&mut self, pipeline: &RenderPipeline, state: &mut RenderState,
                            engine: &TextEngine, layout: &TextLayout, style: &TextStyle,
                            opacity: f32) -> Result<Vec<PositionedGlyph>, TextError>;
}
```

`rasterize_layout` uploads every new glyph of the layout in **one** batch,
so a line of N new glyphs costs one GPU upload rather than N.

### Coverage and compositing

```rust
pub fn rasterize_text_coverage(cache: &mut TextCoverageCache, engine: &TextEngine,
                               layout: &TextLayout, style: &TextStyle,
                               antialias: bool) -> Result<Vec<CoverageMask>, TextError>;
pub fn blurred_text_coverage(cache: &mut TextCoverageCache, engine: &TextEngine,
                              layout: &TextLayout, style: &TextStyle, antialias: bool,
                              blur_level: u8, blur_width: i32) -> Result<Vec<CoverageMask>, TextError>;
pub fn blur_coverage(mask: &CoverageMask, blur_level: u8, blur_width: i32)
    -> Result<CoverageMask, TextError>;
pub fn blit_coverage_into(bitmap: &mut BitmapState, clip: RectI, mask: &CoverageMask,
                          dest_x: i32, dest_y: i32, color: [u8; 4], opacity: u8) -> Option<RectI>;
pub fn compose_text_into(masks: &[CoverageMask], style: &TextStyle, origin: (f32, f32),
                          angle: f32, bitmap: &mut BitmapState, clip: RectI,
                          opacity: u8) -> Result<Option<RectI>, TextError>;
pub fn rasterize_run(backend: GlyphBackend, services: TextRasterServices<'_>,
                     layout: &TextLayout, style: &TextStyle, antialias: bool,
                     opacity: f32) -> Result<RasterizedRun, TextError>;
pub fn project_text(scene: &mut RenderScene, parent: RenderNodeId, atlas: &GlyphAtlas,
                    glyphs: &[PositionedGlyph], start_z: i32) -> Vec<RenderNodeId>;
pub fn compose_text_cpu(atlas: &GlyphAtlas, glyphs: &[PositionedGlyph],
                        width: u32, height: u32) -> Vec<u8>;
```

### Types

`FontId`, `TextStyle`, `TextLayout`, `LaidOutGlyph`, `CoverageMask`,
`PositionedGlyph`, `AtlasGlyph`, `TextRasterServices`, `RasterizedRun`,
`TextError`.

---

## 4. Invariants

* **The layout is the single source of truth for the glyph positions.** The
  `cpu` and `gpu` backends are built from the same `TextLayout`, so the
  advances and metrics agree; only the rasterization and the draw path
  differ. Every script-visible measurement stays in VM pixels.
* **A line's baseline is the face's ascent truncated to whole pixels.** The
  ink box top is `baseline - bitmap_top`; a fractional or vertically centred
  baseline moves every glyph off its row (`reference/cpp/core/visual/FreeType.cpp:494`,
  `reference/cpp/core/visual/impl/LayerBitmapImpl.cpp:911-914`).
* **The colour is not part of the atlas cache key.** The atlas stores
  colour-independent coverage and the colour is applied as a per-node tint,
  so recolouring reuses the cached glyphs.
* **A memoized per-character advance is the rasterizer's own value.** The
  advance memo is keyed by face, character, size and synthetic style, so a
  repeated measurement returns exactly the value a fresh measurement
  returns.
* **A coverage mask is always internally consistent**: `coverage.len() ==
  width * height`, enforced by the constructor.
* **The cached coverage is pen-relative.** The same coverage is reused at any
  pen position; placing it adds the layout's `x`/`baseline_y` and shares the
  buffer instead of copying it.
* **Each glyph keeps its own mask**, so a caller can blur or transform one
  character without bleeding into its neighbours.
* **A blurred shadow is derived from the cached sharp coverage**, so a shadow
  never rasterizes a glyph a second time.
* **With `antialias == false` the coverage is thresholded** to fully
  transparent or fully opaque, matching the aliased coverage the CPU
  compositor blends.
* **The atlas packs glyphs left to right, top to bottom, one pixel apart**, to
  avoid bilinear bleeding; the CPU mirror stays coherent with the GPU so the
  same pixels can be composited for readback.
* **Eviction drops the map entry only.** The shelf packer never reclaims a
  cell, so `GlyphAtlas::clear` is the way to reset the pixel space.
* **A run whose non-whitespace characters all rasterize to no ink is an
  error**, not an empty composite: the resolved face cannot render them.

---

## 5. Data flow

```
L3 native (Layer.drawText / ImageFunction.drawText)
   │  a font face + a string + a style (all VM pixels)
   ▼
TextEngine::layout(font, text, style, wrap_width) ──► TextLayout
   │        (memoized by font + text + size + style + wrap width)
   ▼
rasterize_text_coverage(cache, engine, layout, style, antialias)
   │        → per-glyph 8-bit coverage masks (cached, pen-relative)
   ▼
compose_text_into(masks, style, origin, angle, bitmap, clip, opacity)
   │        → tinted source-over into the layer bitmap
   ▼
bitmap.mark_dirty() ──► engine::Engine::sync_dirty_bitmaps ──► upload_textures
```

The GPU path:

```
rasterize_run(GlyphBackend::Gpu, services, layout, style, antialias, opacity)
   │        → GlyphAtlas::rasterize_layout (fills the atlas, queues the uploads)
   ▼
GlyphAtlas::flush(pipeline, state)      → one upload batch
project_text(render_scene, parent, atlas, glyphs, start_z)
         → one RenderContent::Texture quad per glyph
```

---

## 6. Failure modes

| failure | behaviour |
|---|---|
| an unregistered `FontId` | `TextError::UnknownFont` |
| a zero, negative or non-finite size | `TextError::InvalidSize` |
| unusable font data or a missing collection index | `TextError::InvalidFont` (or `TextError::Glyph`) |
| the host provides no face at all, and no face carries the character a draw needs | the face's default character is drawn in its place; the run continues |
| the requested face carries no glyph at all | `TextError::MissingGlyph` |
| the host's faces cannot be read, and a named family is asked for | `TextError::HostFontScan` |
| the host's faces cannot be read, and a character is drawn | the failure is recorded, and the character is drawn with the face's default character |
| the host's font directories cannot be read, by the scan and by the read that follows it | `TextError::HostFontScan` |
| the atlas has no free cell | `TextError::AtlasFull` — the caller must grow or recreate the atlas; it is never worked around |
| a coverage buffer that does not match its dimensions | `TextError::InvalidCoverage` |
| a blur that would produce an unallocatable mask | `TextError::MaskTooLarge` |
| the L1 rasterizer fails | `TextError::Glyph` |
| the backend rejects a texture | `TextError::Render` |
| a run with no ink for non-whitespace characters | `TextError::Glyph(MissingGlyph)` |

`compose_text_into` returns the union of the drawn glyph rectangles, or
`None` when nothing had ink; `blit_coverage_into` returns the clipped
destination rectangle that received ink, or `None`.

---

## 7. Current limits

* **Glyphs are rasterized at VM resolution**, not at `size · raster_scale()`.
  The visible difference at `s > 1` is softness — a rasterization
  approximation, not a script-observable difference, because every layout
  metric stays in VM units.

---

## 8. Tests

* `crates/engine/tests/text.rs` — the layout, the metrics, the coverage
  masks, the blur and the errors.
* `crates/engine/tests/text_render.rs` — the atlas, the projection and the
  CPU composite.
* `crates/engine/src/text.rs` `mod tests` — the unit-level invariants.
* `crates/compat/tests/font.rs` — the script-visible text surface: the
  measurement, the pre-rendered font's mapping (the file is read at the map)
  and the registered faces.

---

## 9. See also

| topic | document |
|---|---|
| the L3 ↔ L2 protocol for this module | [`rules/api_rules.md`](../../rules/api_rules.md) §4 |
| the L1 font surface | [`arch/l1.md`](../../arch/l1.md) §3 |
| the bitmap the text is composited into | [scene.md](scene.md) |
| the archived protocol trail | [`docs.old/migration/14-l3-l2-protocol.md`](../../../docs.old/migration/14-l3-l2-protocol.md) §6.5 |
