# `Layer.stretchCopyAA` — behavioural specification

**Upstream oracle:** `wtnbgo/layerExAreaAverage`, commit
`b04fbd29548e43d76293625e1d06a3141f0853d8` ("update for linux:
src/plugins/layerExAreaAverage"), the whole plugin in `main.cpp` (210 lines),
the manual in `manual.tjs`, the summary in `readme.txt`.

**Member specified:** `Layer.stretchCopyAA` — the only member the upstream
registers. Nothing else in the upstream tree is script-visible.

**The upstream source is the oracle for this member.** The engine's reference
checkout carries no binary and no source for `layerExAreaAverage.dll`; the
reference's own plugin table names the upstream project
(`reference/krkr2/docs/KRKR_PLUGINS.md:32`), so every citation below points
into the upstream tree. The engine's own `Layer` members (`imageWidth`,
`imageHeight`, `mainImageBuffer`, `mainImageBufferForWrite`,
`mainImageBufferPitch`, `update`) are cited to the reference checkout where
their meaning matters.

This document is prose and formulas. It contains no upstream code.

---

## 1. Registration — how the member reaches the script

The upstream is an NCBIND plugin. When the module is loaded it *attaches* to
the TJS2 class the engine already provides, `Layer`, and adds exactly one
member to it, `stretchCopyAA`, as an instance method (not a static one, not
a property). The upstream's attach block names the class it attaches to and
the member it registers — `main.cpp:207-210`.

* The member is **absent** from `Layer` until the module is linked. A script
  that calls it before the link gets the engine's ordinary "member not found"
  error, not a plugin error.
* The member has **no declared argument-count metadata** in the binding: the
  arity is checked by the member itself (`main.cpp:16-21`). There is no
  overload, no default argument and no alternate spelling.
* The member **returns nothing**. The upstream returns success with the
  result left cleared (`main.cpp:203`); the script sees `void`.

### The script-visible signature

The member is called as `Layer.stretchCopyAA(dleft, dtop, dwidth, dheight,
src, sleft, stop, swidth, sheight)` — nine arguments, in that order.

The upstream's own manual states the same nine-parameter shape
(`manual.tjs:8-26`); the parameter names there are `dleft, dtop, dwidth,
dheight, src, sleft, stop, swidth, sheight`.

| # | name | direction | type | meaning |
|---|------|-----------|------|---------|
| 1 | `dleft` | in | integer | left edge of the destination rectangle, in the destination layer's own image pixels |
| 2 | `dtop` | in | integer | top edge of the destination rectangle, in the destination layer's own image pixels |
| 3 | `dwidth` | in | integer | width of the destination rectangle, in destination image pixels |
| 4 | `dheight` | in | integer | height of the destination rectangle, in destination image pixels |
| 5 | `src` | in | object | the source layer |
| 6 | `sleft` | in | integer | left edge of the source rectangle, in the source layer's own image pixels |
| 7 | `stop` | in | integer | top edge of the source rectangle, in the source layer's own image pixels |
| 8 | `swidth` | in | integer | width of the source rectangle, in source image pixels |
| 9 | `sheight` | in | integer | height of the source rectangle, in source image pixels |

All nine arguments are **required**. Arguments beyond the ninth are ignored
(`main.cpp:21` reads only the first nine).

### Argument coercion

The four destination numbers and the four source numbers are read with the
VM's integer conversion (`main.cpp:40-43`, `main.cpp:60-63`):

* an integer argument is used as it is;
* a real argument is truncated toward zero;
* a string argument is parsed as an integer by the VM's string→integer rule
  (an unparsable string yields 0);
* `void` yields 0;
* an **object** (including `null` and an array) or an **octet** raises the
  VM's conversion error — the upstream does not substitute a value.

`src` is read as an object (`main.cpp:48`): a non-object raises the VM's
"cannot convert to object" error. The upstream does **not** check that `src`
is a `Layer`; it only asks the object for four members (§2).

---

## 2. What the member reads from the two layers

The destination layer — the object the member was called on — is asked for
four members (`main.cpp:29-36`):

* `imageWidth` — the width of the destination layer's own image, in pixels;
* `imageHeight` — the height of the destination layer's own image;
* `mainImageBufferPitch` — the row pitch of that image, **in bytes**;
* `mainImageBufferForWrite` — the address of the image's first pixel, for
  writing.

The source object (`src`) is asked for the same four, except that it is asked
for the **read** address `mainImageBuffer` instead of the write address
(`main.cpp:49-56`).

The reference's meanings for these members:

* `imageWidth` / `imageHeight` are the *image* dimensions, not the layer's
  visible `width` / `height`
  (`reference/cpp/core/visual/LayerIntf.cpp:10796-10800`); an image may be
  larger than the visible rectangle.
* `mainImageBuffer` is the address of row 0 of the layer's **main image**
  (`reference/cpp/core/visual/LayerIntf.cpp:11408-11417`,
  `reference/cpp/core/visual/LayerIntf.cpp:3005-3010`); it is `0` when the
  layer has no image.
* `mainImageBufferForWrite` is the same address, obtained in a way that marks
  the image modified (`reference/cpp/core/visual/LayerIntf.cpp:11420-11433`,
  `reference/cpp/core/visual/LayerIntf.cpp:3012-3018`); it is `0` when the
  layer has no image.
* `mainImageBufferPitch` is the image's pitch in bytes
  (`reference/cpp/core/visual/LayerIntf.cpp:11435-11446`,
  `reference/cpp/core/visual/LayerIntf.cpp:3020-3025`).

---

## 3. Coordinate spaces and pixel formats

* **Destination space.** The destination rectangle is expressed in the
  destination layer's *own image* grid — the same grid `imageWidth` and
  `imageHeight` measure — with the origin at the image's top-left corner.
  The destination pixel of the copy's local column `x` and row `y` is the
  image pixel `(dleft + x, dtop + y)`; its address is the write base plus
  `(dtop + y) * pitch` bytes plus `dleft * 4` bytes
  (`main.cpp:105-106`).
* **Source space.** The source rectangle is expressed in the source layer's
  own image grid, with the origin at that image's top-left corner. The
  visited source pixels are the source image pixels at the *absolute*
  positions `(pixelCol, pixelRow)`: the column index runs over the range derived in
  §5.3, whose first column is `floor((sleft + x*stepX) / 4096)` — an
  absolute column of the source image, not an offset into the source
  rectangle. The upstream reads that pixel at the read base plus
  `pixelRow * pitch` bytes plus `pixelCol * 4` bytes (`main.cpp:128-129`).
* **Both rectangles are integral.** The destination origin and size and the
  source origin and size are all integers. (This differs from the sibling
  `shrinkCopy` plugin, whose destination rectangle may be fractional.)
* **Pixel format.** One 32-bit word per pixel, laid out as `0xAARRGGBB` in a
  little-endian word: alpha in bits 24-31, red in bits 16-23, green in bits
  8-15, blue in bits 0-7. The upstream reads the alpha with a shift of 24 and
  the three colour channels with shifts of 16, 8 and 0
  (`main.cpp:145`, `main.cpp:149-151`), and writes the same layout
  (`main.cpp:183`).
* **Alpha is straight, not premultiplied.** The colour channels are stored
  as they are; the alpha does not scale them in the buffer. The alpha enters
  the *computation* only as a weight on the colour (§5), and the colour the
  member writes is a straight-alpha colour.
* **The province image is never touched.** The upstream reads and writes only
  the main image; it never looks at the province buffer. So after the call
  the destination layer's province still holds what it held before: the newly
  written pixels are *not* made hit-testable, and the province is neither
  resized nor cleared. (The reference exposes the province as a separate
  buffer, `reference/cpp/core/visual/LayerIntf.cpp:11448-11470`.)
* **No blending.** The member writes the averaged colour into the buffer; it
  does not composite with what was there, and it does not consult the
  destination layer's face (`dfFace`) or its hold-alpha flag. Whatever the
  destination pixel held is replaced.

---

## 4. The pipeline, in order

The upstream performs these steps in this order; the order matters for the
error behaviour.

1. Reject a call with fewer than nine arguments (`main.cpp:21`).
2. Read the destination layer's four members (`main.cpp:29-36`).
3. Read the four destination numbers (`main.cpp:40-43`).
4. Read the source object and its four members (`main.cpp:48-56`).
5. Read the four source numbers (`main.cpp:60-63`).
6. **Refuse magnification** (§6).
7. **Clip** the two rectangles to the two images (§7).
8. **Copy**, destination pixel by destination pixel (§5).
9. **Update** the destination layer (§9).
10. **Log** one line (§9).

---

## 5. The copy — exact arithmetic

### 5.1 The fixed-point representation

The upstream measures lengths in a signed integer fixed-point format with
**12 fractional bits**: a stored value `v` denotes the real `v / 4096`.
Integer lengths are converted exactly (multiply by 4096). Real ratios are
converted by multiplying by 4096 and **truncating toward zero**. A stored
value is converted back to an integer by an arithmetic right shift of 12,
which is a floor division. A product of two stored values is converted back
by an arithmetic right shift of 12 (`main.cpp:3-12`).

Everything below is stated in that representation; `4096` is the unit of one
pixel and `4096 * 4096` the unit of one square pixel.

### 5.2 The mapping

Let `destWidth`, `destHeight` be the destination rectangle's size and `srcWidth`,
`srcHeight` the source rectangle's size **after the clipping of §7**.

* The horizontal step (source pixels per destination pixel) is
  `stepX = trunc(4096 * srcWidth / destWidth)`, in stored units
  (`main.cpp:100`).
* The vertical step is `stepY = trunc(4096 * srcHeight / destHeight)`, in stored
  units (`main.cpp:101`).

The source rectangle's top-left corner corresponds to the destination
rectangle's top-left corner. The destination pixel at local `(x, y)` —
`x` in `0 .. destWidth-1`, `y` in `0 .. destHeight-1` — therefore covers the
source region

* horizontally: `[sleft + x*stepX, sleft + (x+1)*stepX)`, and
* vertically: `[stop + y*stepY, stop + (y+1)*stepY)`,

where in this section `sleft` and `stop` denote the source origin in stored
units — the argument multiplied by 4096 — while `x*stepX` and `y*stepY` are
plain products (`main.cpp:98-99`, `main.cpp:110-113`).

Because the step is truncated, the region the destination rectangle covers
inside the source rectangle can fall short of the source rectangle's far edge
by up to `destWidth / 4096` of a pixel (and `destHeight / 4096` of a pixel
vertically). That is the upstream's arithmetic and it is part of the
behaviour.

### 5.3 The source pixel range of one destination pixel

For the destination pixel `(x, y)` the upstream derives, from the covered
region's left and right edges `regionLeft` and `regionRight` (stored units):

* `firstCol = floor(regionLeft / 4096)` — the column that contains the region's left edge;
* `endCol = floor((regionRight + 4095) / 4096)` — the column that contains the region's
  right edge (`main.cpp:116-118`).

and the same for the rows: `firstRow = floor(regionTop / 4096)` and
`endRow = floor((regionBottom + 4095) / 4096)` (`main.cpp:117-119`).

Then two clamps are applied (`main.cpp:120-121`):

* if `endCol` is at or beyond the source image's width, `endCol` becomes the source
  image's width minus one;
* if `endRow` is at or beyond the source image's height, `endRow` becomes the source
  image's height minus one.

The visited source columns are `firstCol, firstCol+1, …, endCol-1` — the end is **exclusive**
(`main.cpp:136`) — and the visited source rows are `firstRow, firstRow+1, …, endRow-1`, also
exclusive (`main.cpp:126`).

> **Consequence (the upstream's dropped edge column/row).** The clamp and
> the exclusive end disagree at the image's far edge: the end index is pulled
> back to the source image's *last* index, and the loop's end is exclusive, so
> **the source image's last column and last row are never sampled at all.**
> Two visible effects:
> * A same-size copy (destination size equal to the source size, same
>   origin) leaves the destination's **last column and last row untouched** —
>   the covered range of those destination pixels is empty, so nothing is
>   written there (§5.6).
> * A downscale whose source rectangle reaches the source image's edge
>   averages the source *without* its last column/row, so the result differs
>   from the ideal area average.
>
> The visited range never leaves the source image, whatever the arguments:
> the end index is at most the image's last index, and the start index is
> never beyond the image's width — when it is, the range is empty and no
> pixel is read.

### 5.4 The overlap of one source pixel

For a visited source pixel `(pixelCol, pixelRow)` the upstream computes the overlap of
that pixel with the covered region, in stored units:

* horizontal overlap `overlapWidth = min((pixelCol+1)*4096, regionRight) - max(pixelCol*4096, regionLeft)`
  (`main.cpp:138-142`);
* vertical overlap `overlapHeight = min((pixelRow+1)*4096, regionBottom) - max(pixelRow*4096, regionTop)`
  (`main.cpp:131-135`);
* the overlap **area** is `overlapArea = (overlapWidth * overlapHeight) >> 12` — i.e. the product of the
  two lengths with 12 fractional bits, floored (`main.cpp:143`).

The area is therefore the covered area in units of 1/4096 of a square pixel
scaled by 4096 — one whole pixel covered entirely is `4096 * 4096 >> 12 =
4096`.

### 5.5 The average

For one destination pixel the upstream keeps these accumulators, all in
stored units (`main.cpp:123-125`):

* `totalArea` — the sum of the areas of the visited source pixels;
* `alphaSum` — the sum of `alpha_i * overlapArea_i` over the visited pixels
  (`main.cpp:146`);
* `colourWeight` — the sum of the colour weights (§below);
* `redSum`, `greenSum`, `blueSum` — the colour sums;
* `transparentArea`, `transparentRedSum`, `transparentGreenSum`,
  `transparentBlueSum` — the same sums for the pixels whose alpha is zero,
  weighted by the plain area instead of the alpha-weighted one
  (`main.cpp:153-158`).

The **colour weight** of a source pixel is

* `colourWeight_i = (overlapArea_i * alpha_i) >> 8` when `alpha_i > 0` (`main.cpp:148`) — note
  the shift is **8**, not 12: the weight is the area scaled by `alpha/256`
  and floored;
* the plain `overlapArea_i` when `alpha_i == 0` — but into the *transparent* sums,
  which do not feed the colour unless nothing else did.

So, for the visited pixels:

* the destination **alpha** is
  `trunc(alphaSum / totalArea)` — the area-weighted mean of the source
  alphas, truncated toward zero (`main.cpp:166`);
* the destination **colour** is
  `trunc(colourSum / colourWeight)` per channel, where the sums and the
  weight total are over the pixels with `alpha > 0` only
  (`main.cpp:149-152`, `main.cpp:179-181`);
* if the alpha-weighted weight total is zero — no pixel with a non-zero alpha
  contributed a non-zero weight — the colour falls back to the **transparent**
  sums: the area-weighted mean of the colours of the pixels whose alpha was
  zero (`main.cpp:167-172`);
* if that fallback's weight total is zero as well, the colour is 0
  (`main.cpp:173-176`).

Consequences worth stating plainly:

* The colour is weighted by **area × alpha** (upstream's `alpha/256`
  approximation), so a nearly transparent pixel barely influences the
  colour, while a transparent pixel does not influence it at all unless every
  other pixel is transparent too.
* The alpha output is weighted by **area only**, so a transparent pixel still
  drags the destination alpha down.
* The colours are averaged in straight-alpha space: the member does not
  premultiply, divide by the summed alpha, or un-premultiply.

### 5.6 What is written

* If the total area of a destination pixel is zero — the covered region
  touches no source pixel — the destination pixel is **left as it was**: the
  upstream skips it (`main.cpp:163`). It is not cleared and not written with
  a zero colour.
* Otherwise the destination pixel is written as one `0xAARRGGBB` word: the
  alpha byte, the red byte, the green byte and the blue byte, each masked to
  eight bits (`main.cpp:183`).

The whole copy is a plain forward loop over the destination rectangle's rows
and columns (`main.cpp:103-107`); the destination is not read, so source and
destination may be the same layer.

---

## 6. The magnification refusal

If the destination rectangle is **wider than the source rectangle, or taller
than it** — `dwidth > swidth` or `dheight > sheight`, compared with the
values as the caller passed them, before any clipping — the member raises a
script error and copies nothing (`main.cpp:65-70`).

The upstream's message, character for character, is
`stretchCopyAA は拡大処理を行なえません。` (that is `stretchCopyAA`, one
ASCII space, then the Japanese text; the upstream file is UTF-8,
`main.cpp:68`). The sentence means "stretchCopyAA cannot perform
magnification".

Notes:

* The check is `>`, not `>=`: a copy whose destination and source rectangles
  have the **same** size is allowed (it is a 1:1 copy, and it is subject to
  the dropped edge column/row of §5.3).
* The check runs **before** the clipping, so a destination that is only
  clipped down to the source's size still raises.
* The check compares the *sizes* only; the origins are irrelevant.

---

## 7. The clipping

After the magnification check the upstream clips the two rectangles to the
two images, in this exact order, and with this exact arithmetic
(`main.cpp:72-96`). Each step shrinks one rectangle and shrinks its partner
**proportionally**, computing the partner's new size from the partner's
**old** size (the right-hand side is evaluated before the assignment).

Let `destImageWidth` / `destImageHeight` be the destination image's dimensions and
`srcImageWidth` / `srcImageHeight` the source image's dimensions.

**Naming used below.** The *arguments* keep the names the manual gives them:
`dleft`, `dtop`, `dwidth`, `dheight` for the destination rectangle and
`sleft`, `stop`, `swidth`, `sheight` for the source rectangle. The *sizes
after the clipping* are named `destWidth`, `destHeight`, `srcWidth` and
`srcHeight`; before any clipping they are simply `dwidth`, `dheight`,
`swidth` and `sheight`.

1. **Destination, right edge.** If `dleft + dwidth > destImageWidth`:
   the destination width becomes `destImageWidth - dleft`, and the source
   width becomes `trunc(swidth * ((destImageWidth - dleft) / dwidth))` — the
   division and the multiplication in double precision, truncated toward
   zero (`main.cpp:73-78`).
2. **Source, right edge.** If `sleft + srcWidth > srcImageWidth`:
   the source width becomes `srcImageWidth - sleft`, and the destination width
   becomes `trunc(destWidth * ((srcImageWidth - sleft) / srcWidth))`, using
   the source width as it was before this step (`main.cpp:79-84`).
3. **Destination, bottom edge.** If `dtop + dheight > destImageHeight`:
   the destination height becomes `destImageHeight - dtop`, and the source
   height becomes `trunc(sheight * ((destImageHeight - dtop) / dheight))`
   (`main.cpp:85-90`).
4. **Source, bottom edge.** If `stop + srcHeight > srcImageHeight`:
   the source height becomes `srcImageHeight - stop`, and the destination
   height becomes `trunc(destHeight * ((srcImageHeight - stop) / srcHeight))`,
   using the source height as it was before this step (`main.cpp:91-96`).

What the clipping does **not** do:

* it never moves an origin (`dleft`, `dtop`, `sleft`, `stop` are unchanged);
* it never clamps a size to zero — a rectangle that lies entirely outside
  its image produces a **negative** size (§8);
* it never clips the left/top edges.

Because the steps are ordered and each one consumes the other's sizes, the
destination and the source rectangles stay in the same ratio as the caller
asked for, to within the truncation of each step.

---

## 8. Edge cases

| situation | upstream behaviour | citation |
|---|---|---|
| fewer than nine arguments | raises the VM's "Invalid argument count" error; nothing is read or written | `main.cpp:21` |
| more than nine arguments | the extra arguments are ignored | `main.cpp:21` |
| `dwidth > swidth` or `dheight > sheight` | raises the magnification error (§6); nothing is written | `main.cpp:65-70` |
| `dwidth == swidth` and `dheight == sheight` (a 1:1 copy) | allowed; the destination's last column and row are left untouched (§5.3) | `main.cpp:116-121`, `main.cpp:136`, `main.cpp:163` |
| a source rectangle of zero width or zero height | the covered region of every destination pixel is empty, so **every destination pixel is left untouched**; the update of §9 still runs. (A zero *destination* size as well makes the scale ratio undefined — a division of zero by zero — but the copy loop writes nothing either way) | `main.cpp:100-101` (a zero step), `main.cpp:163` |
| a destination rectangle of zero width or zero height | the copy loop writes nothing; the update of §9 still runs | `main.cpp:103-107` |
| a source rectangle lying entirely outside the source image | the clipping makes the source size negative and then the destination size negative as well, so the copy loop writes nothing | `main.cpp:79-84`, `main.cpp:103-107` |
| a destination rectangle lying entirely outside the destination image | the clipping makes the destination size zero or negative, so the copy loop writes nothing | `main.cpp:73-78`, `main.cpp:103-107` |
| a destination rectangle partly outside the destination image | the destination rectangle is cut back to the image and the source rectangle is scaled down by the same ratio | `main.cpp:73-96` |
| a source rectangle partly outside the source image | the source rectangle is cut back to the image and the destination rectangle is scaled down by the same ratio | `main.cpp:79-96` |
| a fully transparent source | the destination alpha is 0 and the colour is the area-weighted mean of the transparent pixels' own colours; if those are all zero, the colour is 0 | `main.cpp:153-158`, `main.cpp:167-176` |
| a source with both transparent and opaque pixels | the colour comes from the opaque pixels only; the alpha is the area-weighted mean over *all* the visited pixels, transparent ones included | `main.cpp:146`, `main.cpp:147-158`, `main.cpp:166` |
| a destination pixel that covers no source pixel | the destination pixel keeps its previous content | `main.cpp:163` |
| the source and the destination are the same layer | allowed; the member never reads the destination | `main.cpp:103-107` |
| the source layer has no image (`mainImageBuffer` is 0) | the upstream reads through a null address — undefined behaviour, in practice a crash. The port must not reproduce a crash | `main.cpp:55-56`, `reference/cpp/core/visual/LayerIntf.cpp:3005-3010` |
| the destination layer has no image (`mainImageBufferForWrite` is 0) | the upstream writes through a null address — undefined behaviour, in practice a crash. The port must not reproduce a crash | `main.cpp:35-36` |
| a negative origin (`dleft`, `dtop`, `sleft`, `stop` negative) | the upstream addresses memory outside the image — undefined behaviour. The port must clamp or refuse | `main.cpp:105-106`, `main.cpp:128-129` |
| a negative size | either the magnification check raises (a negative source size makes every positive destination size "larger"), or the copy loop writes nothing, because its bounds are not positive | `main.cpp:65-70`, `main.cpp:103-107` |
| a source or destination image of zero width or height | the copy loop writes nothing | `main.cpp:103-107` |
| the visited source range leaves the source image | never: the end index is clamped to the image's last index, so the visited pixels are inside the image (unless an origin is negative) | `main.cpp:120-121` |

---

## 9. The side effects

* **The destination layer is updated.** After the copy the upstream calls the
  destination layer's `update` member with the destination rectangle's origin
  (`dleft`, `dtop`) and its size **after the clipping** (`destWidth`,
  `destHeight`) (`main.cpp:189-199`). The update hint is 0. The call happens
  even when nothing was written. In the reference, `Layer.update(x, y, w, h)`
  marks the rectangle `(x, y, x+w, y+h)` for update
  (`reference/cpp/core/visual/LayerIntf.cpp:9663-9686`).
* **One log line.** The upstream adds a log entry of the form
  `stretch copy by area average:RESULT (sWidth,sHeight)->(dWidth,dHeight), time = N(ms)`
  where the sizes are the **post-clipping** ones and `N` the elapsed
  milliseconds (`main.cpp:201`). It is a log entry, not a script-visible
  value.
* The member returns nothing to the script.

---

## 10. Worked examples

These are the numbers a conforming implementation must produce. The colour
is written as `0xAARRGGBB`.

**(a) A 4×4 opaque red source, whole image, into a 4×4 destination.**
The copy is a 1:1 copy; by §5.3 the destination's last column and last row
cover an empty source range, so they are **not written**. The result is
three written columns and three written rows of `0xFFFF0000`, and the
remaining seven destination pixels keep whatever they held.

**(b) A 4×4 source whose columns 0-2 are opaque red and whose column 3 is
opaque green, whole image, into a 2×2 destination.**
Every destination pixel comes out `0xFFFF0000` (red). The green column
contributes nothing, because the source rectangle's right edge coincides
with the source image's right edge (§5.3). The ideal area average would have
mixed the green into the right-hand column of the result.

**(c) A 4×2 source whose row 0 holds an opaque red pixel in column 0 and an
`alpha 128` blue pixel in column 1 (the rest of the image is anything), the
source rectangle `(0, 0, 2, 2)`, into a 1×1 destination.**
The result is `0xBFA90055`: alpha 191, red 169, green 0, blue 85.
* the rectangle covers the 2×2 block at the origin, but the source image's
  last row is never sampled (§5.3), so only row 0's two pixels contribute,
  each covering a whole pixel;
* alpha: the mean alpha over those two pixels is `(255 + 128) / 2 = 191.5`,
  truncated to 191;
* colour: the weights are `(4096*255) >> 8 = 4080` for the red pixel and
  `(4096*128) >> 8 = 2048` for the blue one, so red is
  `trunc(255*4080 / (4080+2048)) = 169` and blue is
  `trunc(255*2048 / (4080+2048)) = 85`.

**(d) A 4×2 source whose column 0 holds a fully transparent pixel of the
colour (10, 20, 30) and whose column 1 holds a fully transparent pixel of the
colour (40, 50, 60); the source rectangle `(0, 0, 2, 1)` (one row, two
columns, inside the image), into a 1×1 destination.**
The result is `0x0019232D`: alpha 0, red 25, green 35, blue 45 — the
area-weighted mean of the two *transparent* pixels' colours, which is the
fallback path of §5.5.

**(e) A 2×2 destination image and a destination rectangle `(1, 1, 2, 2)`.**
The destination rectangle is clipped: its width becomes 1 and the source
rectangle is scaled from 4×4 to 2×2. The single written destination pixel is
`(1, 1)` and holds the average of the source's 2×2 block at the origin.

**(f) A 4×4 source into a 3×3 destination with the source rectangle
`(0, 0, 3, 3)`.** Every one of the nine destination pixels is written,
because the covered range of the last destination pixel is non-empty and
lies inside the source image.

Note that the same call with a **4×4** destination raises the magnification
error of §6: the check compares the sizes, and 4 > 3.

---

## 11. The upstream's build system and what a port must decide for itself

### The upstream's build system

* **Build:** CMake. `CMakeLists.txt:1-8` includes the `tp_stub` helper's
  `krkrz.cmake` and calls its `krkrz_plugin` macro with the module name
  `layerExAreaAverage`, the `NCBIND` binding flavour and the single source
  file `main.cpp`. The produced module is `layerExAreaAverage.dll`.
* **Language/binding:** C++ with NCBIND (`ncbind.hpp`), i.e. the plugin
  attaches to an existing TJS2 class rather than declaring a new one.
* **Sources:** one file, `main.cpp` (210 lines). `manual.tjs` is the manual
  (a pseudo-code class declaration, 26 lines) and `readme.txt` a two-line
  summary. **The member `Layer.stretchCopyAA` lives in `main.cpp`.**
* **The tree at the commit this specification was read from**
  (`b04fbd29548e43d76293625e1d06a3141f0853d8`), with the digests a port's
  provenance page records:
  * `main.cpp` — sha256
    `61b7ef796ac284e65032bf1fb57ed91ebe77624ba4ce867201182985cef30813`
  * `manual.tjs` — sha256
    `6a0aa7c9a9ecef136ee736ade7cc722350eccd675598f4da94608f96579f56c1`
  * `readme.txt` — sha256
    `04114912bd21088768cff179d5cfddc94bd1047266ac9cf3151e35978e486176`
  * `CMakeLists.txt` — sha256
    `72895607d68fcaf85d26130b51758d437ee8c10e7bf47bbf498401f4a6394d7e`
* **Licence:** the tree declares none of its own; `readme.txt:12-14` says the
  plugin follows the main engine's, i.e. the KiriKiri Z licence.
* **History that shaped the current behaviour** (the upstream's own git
  history, for the record):
  * `318e7ba` — the destination buffer is taken through the *write* address
    (`mainImageBufferForWrite`) rather than the read one;
  * `0452ad8` — the covered region is now anchored at the source rectangle's
    origin (`sleft`, `stop`) instead of being scaled by the step, and the
    visited range's end is clamped against the source *image*'s size instead
    of the source *rectangle*'s size; before it, a source rectangle that did
    not start at the origin was drawn wrongly;
  * `9942e2d` — the transparent-pixel colour fallback of §5.5 was added.
* **No fixture, no data file, no new format, no device feature.** The plugin
  is pure computation over two layers' pixels. Nothing is read from storage
  and nothing depends on the platform.

### Decisions the specification does not settle

1. **The wording of the magnification error.** The upstream's message is
   Japanese (§6). The engine's own native members raise English messages
   (`crates/compat/src/natives/plugins/shrink_copy/area.rs` for the sibling
   plugin). The port has to choose: keep the upstream's text verbatim, or
   raise an English message of the same kind. This specification fixes the
   *condition* and the *kind* of error, not the text.
2. **The behaviour when a layer has no image.** The upstream dereferences a
   null address. The engine forbids that (a typed error instead). The port
   must raise; the upstream's behaviour here is undefined, so there is
   nothing to be compatible with.
3. **Out-of-range and negative arguments.** The upstream writes outside the
   image buffer. The port must clamp or refuse; the upstream's behaviour is
   undefined, so there is nothing to be compatible with.
4. **Whether the fixed-point arithmetic is reproduced bit-for-bit.** §5.1-§5.5
   state the upstream's 12-bit fixed-point form, the truncations and the
   `>> 8` colour weight, because they are observable (a 1/3 ratio, a
   sub-pixel edge). An implementation that computes the same averages in
   exact rational arithmetic and rounds at the end would differ from the
   upstream in the last bits of some pixels and would, in particular, not
   reproduce the dropped edge column/row. The specification requires the
   upstream's arithmetic.
5. **The dropped edge column/row (§5.3).** It is upstream behaviour and the
   specification requires it. A port that "fixes" it is no longer
   compatible; the writer should note it in the plugin's page.
6. **The update.** The upstream calls the destination layer's `update`
   member. The engine's L2 has its own dirty-region mechanism. The port has
   to decide whether to call the L3 member, to mark the L2 region dirty, or
   both.
7. **The log line.** The upstream writes one log entry per call (§9). The
   port has to decide whether to reproduce it, and where the engine's log
   goes.
8. **The registration timing.** The upstream registers the member when the
   module is linked. The engine's plugin registry does the same for every
   built-in plugin, so this is a convention rather than a decision — but the
   writer has to add the module to the engine's built-in table under the DLL
   name `layerExAreaAverage.dll`.

---

## 12. Citation index

Every claim in this document is anchored to one of these.

| upstream location | what it establishes |
|---|---|
| `main.cpp:3-12` | the 12-bit fixed-point representation and its conversions |
| `main.cpp:14-19` | the member is a static native method of the plugin's class |
| `main.cpp:16-21` | the member's name, its arity check and the "fewer than nine arguments" error |
| `main.cpp:29-36` | the four destination members read, and the write address |
| `main.cpp:40-43` | the four destination numbers and their integer coercion |
| `main.cpp:48-56` | the source object and the four source members, with the read address |
| `main.cpp:60-63` | the four source numbers and their integer coercion |
| `main.cpp:65-70` | the magnification refusal and its message |
| `main.cpp:72-96` | the four clipping steps, their order and their arithmetic |
| `main.cpp:98-101` | the source origin and the two truncated scale steps |
| `main.cpp:103-107` | the copy loop over the destination rectangle; the destination pixel address |
| `main.cpp:110-113` | the covered source region of one destination pixel |
| `main.cpp:116-121` | the source pixel range and the two clamps |
| `main.cpp:123-125` | the accumulators |
| `main.cpp:126`, `main.cpp:136` | the exclusive row and column ends of the visited range |
| `main.cpp:128-129` | the source pixel address |
| `main.cpp:131-135`, `main.cpp:138-143` | the vertical and horizontal overlaps and the area |
| `main.cpp:144-146` | the total area and the alpha sum |
| `main.cpp:147-152` | the alpha-weighted colour sums and their weight total |
| `main.cpp:153-158` | the transparent-pixel sums |
| `main.cpp:163` | the destination pixel that covers nothing is left untouched |
| `main.cpp:166` | the destination alpha |
| `main.cpp:167-176` | the transparent fallback and the zero-colour case |
| `main.cpp:179-181` | the destination colour channels |
| `main.cpp:183` | the written pixel word |
| `main.cpp:189-199` | the `update` call on the destination layer |
| `main.cpp:201` | the log line |
| `main.cpp:203` | the member returns nothing |
| `main.cpp:207-210` | the attach to the `Layer` class and the registered member name |
| `manual.tjs:8-26` | the documented nine-parameter signature |
| `readme.txt:1-10` | the plugin's purpose and licence |
| `CMakeLists.txt:1-8` | the build system and the module name |

Reference-checkout citations (the engine's own members, not the plugin's):

| reference location | what it establishes |
|---|---|
| `reference/cpp/core/visual/LayerIntf.cpp:10796-10800` | `imageWidth` is the image width |
| `reference/cpp/core/visual/LayerIntf.cpp:11408-11417` | `mainImageBuffer` is the main image's row 0 |
| `reference/cpp/core/visual/LayerIntf.cpp:11420-11433` | `mainImageBufferForWrite` marks the image modified |
| `reference/cpp/core/visual/LayerIntf.cpp:11435-11446` | `mainImageBufferPitch` is in bytes |
| `reference/cpp/core/visual/LayerIntf.cpp:11448-11470` | the province buffer is separate from the main image |
| `reference/cpp/core/visual/LayerIntf.cpp:3005-3025` | a layer without an image answers 0 for the buffer and the pitch |
| `reference/cpp/core/visual/LayerIntf.cpp:9663-9686` | `Layer.update(x, y, w, h)` |
| `reference/cpp/core/tjs2/tjsError.h:268` | the "Invalid argument count" message of the bad-parameter-count error |
| `reference/cpp/core/tjs2/tjsVariant.h:694-699` | a non-object argument raises the conversion error |
| `reference/cpp/core/tjs2/tjsVariant.h:862-877` | the integer coercion rules (void, real, string, object, octet) |
| `reference/cpp/core/tjs2/tjsError.h:250-253` | the wording of the conversion errors |
| `reference/krkr2/docs/KRKR_PLUGINS.md:32` | the upstream project of `layerExAreaAverage.dll` |
