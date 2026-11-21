# shrinkCopy (`shrinkCopy.dll`)

**DLL name:** `shrinkCopy.dll` — linked with `Plugins.link("shrinkCopy.dll")`
**Rust module:** `crates/compat/src/natives/plugins/shrink_copy/`
**Reference:** the plugin's own source — `main.cpp` and `manual.tjs`
**Registers:** two members on `Layer`

The plugin is a third-party one: it is not part of the reference engine's own
tree, but the reference's plugin list names its repository
(`reference/krkr2/docs/KRKR_PLUGINS.md:45`). Its source is the oracle for
the two members below, and the manual that ships with it states their
behaviour. The provenance and the licence are recorded in
[Provenance and licence](#provenance-and-licence) below.

| Reference file | What it contributes |
|---|---|
| `main.cpp` | the two members, the area average and the box average |
| `manual.tjs` | the arguments, the refusal of an enlargement, the ignored face and hold-alpha, the ignored alpha of the fast copy and the resize |

The plugin registers no class of its own: it attaches two instance members to
the core `Layer` class (`main.cpp:369`, `:485`), the same shape the
`layerEx` family uses. The plugin index is [`../plugins.md`](../plugins.md).

## Provenance and licence

The plugin's tree, its licence and what the port took from it are recorded
here.

| what | value |
|---|---|
| upstream project | `wamsoft/shrinkCopy`, byte-identical to `wtnbgo/shrinkCopy` (the project the reference's plugin list names), branch `master` |
| source consulted | `main.cpp`, sha256 `ea8c9b43f82c21eb600b26ec0393e9cf6dacd4bb3a7b2359549a3bd53eee68d6` |
| manual consulted | `manual.tjs`, sha256 `3044b65ee3b3fd099d2bc74cbe91e56c34e50e1226c1b6443e9f92d0d4b7bb8f` |
| readme consulted | `readme.txt`, sha256 `333f1679e74066be5705fefeaf49060a570266b05ceb2f1d0f28efe92102d1bc` |
| licence | the tree declares **no licence of its own**; its `readme.txt` says the plugin follows the engine's, the **KiriKiri Z licence** — a custom, BSD-style, non-copyleft licence (`reference/krkr2/LICENSE:13-32`) |
| notice | `crates/compat/src/natives/plugins/shrink_copy/LICENSE.krkr2`, the engine's licence file byte for byte (sha256 `31887f008d3666961adb51572add59709c12eb56d39066fed62b5267d380042d`, the same file the vendored engine tree keeps in `crates/tjs/cpp/tjs2/LICENSE.krkr2`) |

The KiriKiri Z licence permits use and redistribution in source and binary
form, whether modified or not, provided the copyright notice, the conditions
and the disclaimer are kept (in the source, and in the materials that
accompany a binary), and provided the organisation's and the contributors'
names are not used to advertise or promote the sale of a derived product. It
carries no patent grant and no patent clause. This repository is
GPL-3.0-only, which may incorporate permissive code with the permissive terms
still applying to that part, so the two are compatible and this repository's
licence does not change.

**This is a derived work, not a clean-room rewrite.** The implementation
*reads* the upstream's arithmetic rather than re-deriving it: the area
average's weights, the 1/256-pixel resolution, the way the edge spans fold
the part that reaches past the source rectangle onto the boundary pixel, the
clipping of both rectangles, the truncation of every division and the
remainder handling of the fast copy all follow `main.cpp` line by line. The
manual alone does not specify those parts — it states the arguments, the
refusal of an enlargement, the ignored face and hold-alpha, the ignored
alpha of the fast copy and the resize — so the sources are the only oracle
for the pixels, and they are a better one than a fixture because they
explain the arithmetic instead of only its result.

What was taken from the upstream:

* the argument lists, the arity and the error conditions of both members;
* the parameter check, the clipping of the source and the destination
  rectangles and the no-op case;
* the area average: the span tables, the 1/256-pixel resolution, the nine
  weighted regions of one destination pixel, the separate weighting of the
  colour and the alpha, and the divisor;
* the box average: the two-step (horizontal, then vertical) truncation, the
  remainder pixel of a row and of a column, the opaque alpha and the
  resize.

What was not taken:

* the raw-buffer access. The upstream reads and writes the layer's 32-bit
  ARGB main-image buffer through a pointer and a pitch; the port reads and
  writes the engine's straight-alpha RGBA image, and the same pixels are
  averaged;
* the memory handling. The upstream allocates the span tables with `malloc`
  and frees them again; the port uses ordinary vectors;
* the registration. The upstream attaches the members through the plugin
  binding's own macros; the port registers them with the engine's
  `register_instance_members`, the same call every other plugin uses.

The behaviour test cites the upstream's source as its oracle:
`crates/compat/tests/shrink_copy.rs` names the `main.cpp` line that defines
what each test pins.

## Surface

| member | kind | behaviour |
|---|---|---|
| `shrinkCopy(dleft, dtop, dwidth, dheight, src, sleft, stop, swidth, sheight)` | instance method | copies a rectangle of `src` into a smaller rectangle of the layer, averaging the area every destination pixel covers |
| `shrinkCopyFast(src, shrink_x, shrink_y)` | instance method | averages whole boxes of `shrink_x` by `shrink_y` source pixels and resizes the layer to the size of the result |

Both members exist only after the first `Plugins.link("shrinkCopy.dll")`:
the reference attaches them when the module loads (`main.cpp:369`, `:485`),
and the port registers them from the built-in table's `register` call, like
every other plugin.

## Behaviour

### `shrinkCopy` — the area average

The destination rectangle `(dleft, dtop, dwidth, dheight)` is mapped onto
the source rectangle `(sleft, stop, swidth, sheight)`, and every
destination pixel averages the source area it covers. The copy only
shrinks: a destination rectangle wider or taller than the source rectangle
is refused (`main.cpp:108-118`, `manual.tjs:23`).

The average is taken at a **1/256-pixel resolution** (`main.cpp:233-259`):
the covered area of every source pixel is a fraction of that resolution, so
the result is an approximation of the exact area average. The plugin's own
readme says so (`readme.txt:13`).

The area of one destination pixel is covered by nine regions — four
corners, four edges and the middle — and every region is weighted by how
much of the source pixel it covers (`main.cpp:187-226`). The colour
channels and the alpha are weighted separately: the alpha of a source pixel
that the destination pixel only partly covers counts only in part, so the
alpha is averaged with the colour and a transparent source pixel pulls the
destination's alpha down.

The first and the last destination pixel of an axis are *edge* pixels
(`main.cpp:284-301`): their span reaches past the source rectangle, and the
part that reaches past it is folded onto the boundary pixel — the colour
keeps the weight of the whole span, the alpha only the weight of the part
that is inside the rectangle. The middle pixels are *inner* pixels
(`main.cpp:271-282`): their span lies inside the source rectangle and every
source pixel of the span is weighted by its covered fraction.

### The clipping

Both rectangles are clipped before the copy (`main.cpp:122-180`):

* the source rectangle is clipped to the source image. The part that is cut
  away shrinks the destination rectangle with it, and the destination
  rectangle moves by the same amount, so the copy stays aligned with the
  source pixels that are left;
* the destination rectangle is clipped to the destination image. The pixels
  outside the destination image are not written.

A rectangle that covers no pixel at all is not an error: the copy does
nothing and the destination keeps its content (`main.cpp:126-129`).

### `shrinkCopyFast` — the box average

The source is averaged over whole boxes of `shrink_x` by `shrink_y` pixels
(`main.cpp:405-434`). The horizontal average is taken first and the
vertical one second, and each of them truncates, so the result is not always
the mean of the box.

The copy **resizes the destination layer** to the size of the result: the
width becomes the source's width divided by `shrink_x` (rounded up) and the
height its height divided by `shrink_y` (rounded up) — the same as
`setImageSize(ceil(src.imageWidth / shrink_x), ceil(src.imageHeight /
shrink_y))` (`main.cpp:398-404`, `manual.tjs:41-42`). The pixels that do
not fill a whole box become one more pixel of the result (`main.cpp:467-470`),
so the last column and the last row can look stretched.

The vertical factor is optional: a missing or zero value repeats the
horizontal one (`main.cpp:372-381`, `manual.tjs:36`).

The alpha is **not read**: every destination pixel comes out opaque
(`main.cpp:435-465`, `manual.tjs:38`).

### The face and the hold-alpha flag

Neither member consults the layer's draw face or its hold-alpha flag
(`manual.tjs:25`): both write the destination image directly, and neither
blends. The port does the same — the pixels are written into the
destination's main image, and no face or blend is consulted.

## State and lifetime

No state: the module keeps nothing between calls. The members read the
source image, write the destination image and mark the destination's pixels
as changed, so the frame uploads it.

## Errors

| condition | outcome |
|---|---|
| `shrinkCopy` is called with fewer than nine arguments | typed error: `Layer.shrinkCopy requires 9 arguments` (`main.cpp:81`) |
| the source rectangle is empty, or the destination rectangle is not smaller than the source rectangle | typed error: `Layer.shrinkCopy: the source rectangle must cover the destination rectangle` (`main.cpp:110-112`) |
| the source is not a layer, or the layer has no main image | typed error: `Layer.shrinkCopy: the source is not a Layer` / `… the source layer has no main image` |
| `shrinkCopyFast` is called with fewer than two arguments | typed error: `Layer.shrinkCopyFast requires 2 arguments` (`main.cpp:377`) |
| a shrink factor is not positive | typed error: `Layer.shrinkCopyFast: the shrink factors must be at least 1` (`main.cpp:392-395`) |
| the destination layer has no main image | typed error: `Layer.shrinkCopyFast: the destination layer has no main image` |
| the destination image cannot be resized | typed error: `Layer.shrinkCopyFast: could not resize the destination image` (`main.cpp:398-404`) |

A rectangle that covers no pixel is not an error in either member: the copy
does nothing.

## Tests

| file | what it pins |
|---|---|
| `crates/compat/tests/shrink_copy.rs` | that the link resolves to the built-in and the members appear with it; the arithmetic of the area average and of the alpha average; the clipping of the source and of the destination rectangle; the refusal of an enlargement; the arity of both members; the resize, the box average and the opaque alpha of the fast copy |

## Known gaps

* The reference reads the source's raw main-image buffer, a 32-bit ARGB
  buffer whose pitch may be wider than the image. The port reads the
  engine's straight-alpha RGBA image, so a layer whose image is stored with
  a padded pitch is averaged over the same pixels — the arithmetic is
  unchanged.
* The reference's `shrinkCopyFast` calls the *script* member `setImageSize`
  on the destination (`main.cpp:400-403`), so a script that overrides it
  changes the size the fast copy produces. The port resizes the
  destination through the engine's layer image size — the same call the
  `Layer.setImageSize` member makes (`Scene::internal_set_image_size`), so
  the layer's image size, its rectangle and its main image all follow the
  result's size — and the size is the one the manual states.
* The reference's `shrinkCopyFast` reads the source buffer it took before
  the resize and writes into the buffer the resize reallocated. The port
  reads the source's pixels before the resize, so a copy of a layer onto
  itself averages the image as it was before the resize.
* The plugin's own manual notes that the area average can leave a false
  colour at the border between a transparent and an opaque area
  (`manual.tjs:27-28`), because the colour of a partly covered source pixel
  is weighted as if the pixel were covered whole. The port keeps that
  weighting, so the artefact is the same.
