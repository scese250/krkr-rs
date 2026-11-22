# `layerExSave` — behavioural specification of the analytic members

A clean-room specification written from the upstream C++ source. It contains
behaviour and formulas only: no upstream code, no upstream control flow, and
no upstream internal identifier used as a name for anything but the upstream's
own public entry point (the script-visible member names).

## 0. Provenance and scope

| | |
|---|---|
| upstream repository | `https://github.com/wtnbgo/layerExSave` |
| commit read | `b0939dc916f1b39775768cbb25e6a54b4e217410` ("manual: getCropRect の説明の分断を修正 / threthold → threshold / octetVectorSum の @param を分ける") |
| upstream file for every analytic member | `utils.cpp` |
| upstream file for the three `Window` members | `Main.cpp` |
| upstream's own manual (pseudo-code) | `manual.tjs` |
| DLL name in the reference's plugin table | `LayerExSave.dll` (`reference/krkr2/docs/KRKR_PLUGINS.md:12`) |
| upstream's own build name for the module | `layerExSave` (`CMakeLists.txt:6`) |

The upstream attaches its members to the **host engine's own classes**: the
analytic members are attached to `Layer` and `Math` (`utils.cpp:151`, `:195`,
`:258`, `:317`, `:421`, `:519`, `:552`, `:618`, `:674`, `:724`, `:803`), and the
save trio is attached to `Window` as an instance class with a per-window native
payload (`Main.cpp:356-360`). There is no new script-visible class.

**In scope** (the members this specification covers):

- `Layer.clearAlpha`, `Layer.oozeColor`, `Layer.getAverageColor`,
  `Layer.getCropRect`, `Layer.getCropRectZero`, `Layer.getDiffPixel`,
  `Layer.getDiffRect`, `Layer.getFingerPrintValue`,
  `Layer.getShrinkVectorOctet`, `Layer.isBlank`
- `Math.octetVectorSum`
- `Window.startSaveLayerImage`, `Window.stopSaveLayerImage`,
  `Window.cancelSaveLayerImage`

**Out of scope** (see §15): `Layer.saveLayerImagePng`,
`Layer.saveLayerImagePngOctet`, `Layer.saveLayerImageTlg5`,
`Layer.saveProvinceImage` — the PNG and TLG5 encoders. They need a file
format the tree does not have, so they are a separate task with a fixture.
`Layer.copyBlueToAlpha` (`utils.cpp:465`) is also out of scope: it is recorded
under a different row of the engine's own work list.

The upstream's own manual describes the members in Japanese pseudo-code
(`manual.tjs`); it is cited below where it states a default or an intent, but
where the manual and the code disagree the **code** is the oracle (the manual
says `Math.octetVectorSum(a, b = void)` at `manual.tjs:171`, while the code
requires two arguments — see §13).

## 1. The host pixel model

Every analytic member except `Math.octetVectorSum` works on the **layer's own
main image** (`dfMain`) — never on the province image, never on the desktop
or the screen. The upstream reaches it through the host's own layer
properties: `hasImage`, `imageWidth`, `imageHeight`, `mainImageBufferPitch`
(row pitch in bytes) and `mainImageBuffer` / `mainImageBufferForWrite` (the
pixel address; the writable form also marks the image modified)
(`utils.cpp:21-49`, `:52-75`).

The pixel is 32 bits, four 8-bit channels, and the upstream addresses the
channels **by their offset in the buffer**:

| buffer offset | channel |
|---|---|
| 0 | blue |
| 1 | green |
| 2 | red |
| 3 | alpha |

That order is the host's own ARGB pixel layout on a little-endian machine
(the reference's pixel structure declares `b, g, r, a` in that order,
`reference/krkr2/cpp/core/visual/argb.h:26-31`), and the upstream's PNG writer
confirms it by emitting the buffer bytes as `R = byte 2`, `G = byte 1`,
`B = byte 0`, `A = byte 3` (`savepng.cpp:51-57`).

**This matters for the port.** The engine's own layer image is a
straight-alpha **RGBA** buffer (`crates/engine/src/scene.rs:386-418`: "Straight
alpha RGBA8 pixels"), so in the engine the same channels sit at different
offsets: red at 0, green at 1, blue at 2, alpha at 3. The mapping from the
upstream's offsets to the engine's is therefore:

| upstream offset | channel | engine offset |
|---|---|---|
| 0 | blue | 2 |
| 1 | green | 1 |
| 2 | red | 0 |
| 3 | alpha | 3 |

Everything below is stated in **channels**, so it is unambiguous; where the
result is a packed word, the packing is stated explicitly.

Alpha is 8-bit, 0 = fully transparent, 255 = opaque. The upstream never
adjusts a channel for the alpha: it adds, averages and compares the stored
bytes as they are. A port must do the same — the members are byte arithmetic
over the stored image, not a colour-space conversion.

Coordinates are **layer-image pixels**: the origin is the top-left of the
layer's own main image, x grows right, y grows down. They are not desktop
coordinates and not layer-tree coordinates.

## 2. Shared conventions

### 2.1 The image precondition

Every member that touches pixels first requires that the receiver is a
`Layer` instance, that it has an image (`hasImage` is true), that the image
is non-empty (`imageWidth > 0` and `imageHeight > 0`) and that the row pitch
is non-zero (`utils.cpp:21-49`). When the requirement fails, the member
**raises a script exception** whose message is:

- `"Invalid layer image."` for `getCropRect`, `getCropRectZero`,
  `getDiffRect`, `getDiffPixel`, `oozeColor`, `getFingerPrintValue` and
  `getShrinkVectorOctet` (`utils.cpp:132`, `:176`, `:235`, `:298`, `:359`,
  `:633`, `:705`);
- `"src must be Layer."` for `isBlank` (`utils.cpp:482`) and for
  `getAverageColor` (`utils.cpp:569`) — the upstream reuses the source-side
  wording for the receiver;
- `"dest must be Layer."` for `clearAlpha` (`utils.cpp:535`).

A second layer passed as an argument (the *base* of the diff members) that is
not a `Layer` instance, or that has no image, produces the same
`"Invalid layer image."` failure (`utils.cpp:233-235`, `:296-298`).

### 2.2 The two error kinds

Besides the thrown messages above, the upstream uses the VM's own status
codes, which surface in a script as an error rather than as a message:

- **bad parameter count** — raised when a member is called with fewer
  arguments than it requires: `getDiffRect` (`utils.cpp:226`), `getDiffPixel`
  (`utils.cpp:278`), `isBlank` (`utils.cpp:474`), `getAverageColor`
  (`utils.cpp:561`), `oozeColor` (`utils.cpp:341`), `Math.octetVectorSum`
  (`utils.cpp:781`).
- **invalid parameter** — raised when the argument *values* are unusable:
  `Math.octetVectorSum` (`utils.cpp:784`) and `getShrinkVectorOctet`
  (`utils.cpp:700`).

Extra arguments beyond what a member reads are ignored.

### 2.3 The region clipping rule

`isBlank` and `getAverageColor` take a rectangle in layer-image pixels and
clip it to the image before use (`utils.cpp:491-495` and `:578-582`, the same
rule twice). Given `left`, `top`, `width`, `height` and an image of
`sw × sh`:

- if `left < 0` then `width` is reduced by `left` and `left` becomes 0;
- if `top < 0` then `height` is reduced by `top` and `top` becomes 0;
- if `left + width - sw > 0` then `width` is reduced by that excess (so the
  right edge is clamped to `sw`);
- if `top + height - sh > 0` then `height` is reduced by that excess.

A **negative** width or height is not repaired: it survives the clipping. The
two members then differ in what they do with it (§9 and §10).

The clipping never widens the rectangle: a rectangle that starts outside the
image on the left is *cut*, not moved.

### 2.4 The "same colour" rule

The diff members compare two pixels with one rule (`utils.cpp:200-201`): two
pixels are the **same** when their alpha bytes are equal **and** either that
alpha is 0 (in which case the colour bytes are not looked at) or all three
colour bytes are equal. In channel terms: *same ⇔ alpha equal ∧ (alpha = 0 ∨
red, green and blue all equal)*.

A pixel with alpha 0 therefore compares equal to any other pixel with alpha 0,
whatever its stored colour.

## 3. `Layer.getCropRect`

**Signature.** `Layer.getCropRect()` — an instance method on `Layer`, no
arguments, no optional arguments. Attached to the host `Layer` class at
`utils.cpp:151`; described at `manual.tjs:83-87`.

**Result.** A TJS **Dictionary** with the four integer members `x`, `y`, `w`,
`h` (`utils.cpp:98-107`), or **void** when the whole image is transparent.

**Semantics.** The bounding box of the image's *non-transparent* pixels: a
pixel counts when its **alpha byte is not 0**; the colour bytes are not
looked at (`utils.cpp:112-117`).

Let the image be `w × h`. Scan the columns left to right for the first
column that holds at least one pixel with alpha ≠ 0; call its index `x1`. If
no column holds one, the image is entirely transparent and the result is
**void** (`utils.cpp:138-139`). Otherwise scan the columns right to left for
the last column holding such a pixel, `x2`. The width is `x2 - x1 + 1`
(`utils.cpp:140-141`). Then, restricted to the columns `x1 … x2`, scan the
rows top to bottom and bottom to top for the first and last row holding a
pixel with alpha ≠ 0, `y1` and `y2` (`utils.cpp:142-143`). The result is
`x = x1`, `y = y1`, `w = x2 - x1 + 1`, `h = y2 - y1 + 1`
(`utils.cpp:146`).

**Edge cases.**

- An entirely transparent image → **void** (not a dictionary, not null).
- An image with a single non-transparent pixel → a 1 × 1 rectangle at that
  pixel.
- The rectangle is the bounding box of the content, so it may be strictly
  smaller than the image.
- The `w` and `h` of the result are always ≥ 1 when a dictionary is returned.

**Errors.** The image precondition of §2.1 (`"Invalid layer image."`).

**Coordinates.** Layer-image pixels.

## 4. `Layer.getCropRectZero`

**Signature.** `Layer.getCropRectZero()` — an instance method on `Layer`, no
arguments. Attached at `utils.cpp:195`; described at `manual.tjs:89-93`.

**Result.** A TJS Dictionary with `x`, `y`, `w`, `h` (integers), or **void**
when the whole image is zero.

**Semantics.** The same bounding-box computation as `getCropRect`, with a
different notion of "content": a pixel counts when **any of its four bytes is
non-zero** — that is, when the pixel is not the all-zero word
`0x00000000` (`utils.cpp:156-161`). The transparent-but-coloured pixel
(alpha 0, colour non-zero) therefore *is* content here, while it is not
content for `getCropRect`.

The scans and the result are exactly those of §3: the first and last column
holding such a pixel, then the first and last row holding such a pixel
within those columns (`utils.cpp:182-187`), and the same
`x, y, w = x2 - x1 + 1, h = y2 - y1 + 1` result (`utils.cpp:190`).

**Edge cases.**

- An image whose every byte is zero → **void**.
- An image that is entirely transparent but has coloured pixels → a
  dictionary covering those pixels (unlike `getCropRect`, which answers
  void).

**Errors.** `"Invalid layer image."` (§2.1).

## 5. `Layer.getDiffRect`

**Signature.** `Layer.getDiffRect(base)` — an instance method on `Layer`.
`base` (in) is the *base* image: another `Layer` whose main image must have
the same dimensions as the receiver's. Attached at `utils.cpp:258`;
described at `manual.tjs:95-101`.

**Result.** A TJS Dictionary with `x`, `y`, `w`, `h`, or **void** when the
two images are pixel-identical under the "same colour" rule of §2.4.

**Semantics.** The bounding box of the pixels where the receiver's image and
`base`'s image differ, computed exactly like §3 but with "content" replaced
by "differs" (`utils.cpp:206-212`, `:245-250`).

The scan order is: first column (left to right) holding a differing pixel,
then last column (right to left), then — restricted to those columns — the
first row (top to bottom) and the last row (bottom to top) holding a
differing pixel. The result is `x = x1`, `y = y1`, `w = x2 - x1 + 1`,
`h = y2 - y1 + 1` (`utils.cpp:253`).

A pixel pair differs unless the "same colour" rule of §2.4 says they are the
same: alpha unequal → different; alpha equal and non-zero → different when
any colour channel differs; alpha equal and zero → **never** different.

**Edge cases.**

- Identical images → **void**.
- The receiver and `base` may be the same object → always **void**.
- A one-pixel difference → a 1 × 1 rectangle at that pixel.

**Errors.**

- Fewer than one argument → bad parameter count (`utils.cpp:226`).
- `base` is not a `Layer`, or either image is missing → `"Invalid layer
  image."` (`utils.cpp:233-235`).
- The two images have different `imageWidth` or `imageHeight` →
  **`"Different layer size."`** (`utils.cpp:238-239`). The sizes are compared
  as they are; the images are never resized to match.

## 6. `Layer.getDiffPixel`

**Signature.** `Layer.getDiffPixel(base, samecol, diffcol)` — an instance
method on `Layer`.

- `base` (in) — the base image; a `Layer` with the same dimensions as the
  receiver.
- `samecol` (in, optional, default: *none*) — the colour written over the
  pixels that are the same in both images. A **0xAARRGGBB** colour word.
- `diffcol` (in, optional, default: *none*) — the colour written over the
  pixels that differ. A **0xAARRGGBB** colour word.

Attached at `utils.cpp:317`; described at `manual.tjs:103-110`.

**Result.** The **number of differing pixels**, as an integer.

**Semantics.** The two images are compared pixel by pixel with the "same
colour" rule of §2.4 (`utils.cpp:307`). Every differing pixel increments the
count. If `samecol` was supplied (i.e. the argument was present and not
`void`), every *same* pixel of the **receiver's** image is overwritten with
that colour; if `diffcol` was supplied, every *differing* pixel of the
receiver's image is overwritten with that colour (`utils.cpp:308-309`).

The comparison is done against the pixels as they are read, and each pixel is
visited once, so the fill never feeds back into a later comparison. The
receiver's image is the one that is written; `base` is never modified.

The colour word is stored **verbatim** into the pixel: the four bytes of
0xAARRGGBB become the four channels (alpha = the word's top byte, red, green,
blue below it). No premultiplication or other adjustment is applied — the
member writes the given 8-bit values as they are.

**Edge cases.**

- With no `samecol`/`diffcol` the member is a pure comparison and returns the
  count.
- Passing `void` explicitly for a colour argument means "do not fill", exactly
  like omitting it (`utils.cpp:280-288`).
- A colour word with an alpha of 0 still writes all four bytes, so the pixel
  becomes transparent with that stored colour.
- The count is 0 for identical images.

**Errors.**

- Fewer than one argument → bad parameter count (`utils.cpp:278`).
- `base` is not a `Layer`, or either image is missing → `"Invalid layer
  image."` (`utils.cpp:296-298`).
- Different image sizes → **`"Different layer size."`** (`utils.cpp:301-302`).

## 7. `Layer.clearAlpha`

**Signature.** `Layer.clearAlpha(threshold = 0, fillColor = 0)` — an instance
method on `Layer`.

- `threshold` (in, optional, default `0`) — the alpha threshold.
- `fillColor` (in, optional, default `0`) — the colour written into the
  cleared pixels, as a **0xRRGGBB** word (no alpha: the cleared pixel is
  fully transparent).

Attached at `utils.cpp:552`; described at `manual.tjs:132-137`.

**Semantics.** Every pixel of the receiver's own image whose **alpha is less
than or equal to** `threshold` is replaced by the fill colour with its alpha
cleared to 0 (`utils.cpp:539-548`). A pixel that survives keeps all four of
its bytes.

The fill colour is masked to its low 24 bits (`utils.cpp:529`), so only the
red, green and blue parts of the argument are used and the alpha part of the
argument is discarded. The written pixel is: alpha 0, red = `(fillColor >>
16) & 0xff`, green = `(fillColor >> 8) & 0xff`, blue = `fillColor & 0xff`.
This is the same "verbatim" rule as §6.

The comparison is `alpha ≤ threshold`, inclusive (`utils.cpp:542`).

**Edge cases.**

- The default call `clearAlpha()` therefore rewrites every fully transparent
  pixel to the all-zero word — a transparent pixel that still carries colour
  loses it.
- `threshold` is **not clamped**: any integer is accepted and compared as an
  integer. A threshold of 255 clears every pixel whose alpha is ≤ 255, that
  is, every pixel. A negative threshold clears nothing (no alpha is ≤ a
  negative number).
- `threshold` is read as a 32-bit integer: a larger script integer is
  truncated to 32 bits before the comparison (`utils.cpp:528`).
- `fillColor` is masked to 24 bits; a negative argument contributes its
  two's-complement low 24 bits.
- The member takes no argument-count check: calling it with no argument at
  all is legal and means `threshold = 0`, `fillColor = 0`.

**Errors.** `"dest must be Layer."` when the image precondition fails
(`utils.cpp:534-536`).

## 8. `Layer.oozeColor`

**Signature.** `Layer.oozeColor(level, threshold = 1, fillColor = 0)` — an
instance method on `Layer`.

- `level` (in, required) — how many dilation steps to perform.
- `threshold` (in, optional, default `1`) — the alpha threshold: pixels
  whose alpha is **below** it are the ones that get stretched.
- `fillColor` (in, optional, default `0`) — the colour the below-threshold
  pixels are cleared to before the stretching, as a **0xRRGGBB** word.

Attached at `utils.cpp:421`; described at `manual.tjs:112-118`. The upstream's
own comment for the threshold is garbled by an unresolved merge marker in the
source (`utils.cpp:330-334`); the code is unambiguous.

**Semantics.** This is the "edge colour bleed" that stops a downscaled image
from showing false colours at its border: the colour of the opaque part is
pushed outwards into the nearly transparent part.

**Step 1 — classify and clear** (`utils.cpp:367-378`). Every pixel is
examined once:

- if its **alpha ≥ threshold**, the pixel is *solid*: it keeps its colour and
  it becomes a donor;
- otherwise the pixel is *empty*: its red, green and blue are overwritten
  with the fill colour (its alpha is **not** touched) and it becomes a
  candidate to receive a colour.

**Step 2 — stretch**, `level` times (`utils.cpp:381-411`). In each round, the
image is walked top to bottom and left to right. A pixel that is still
*empty* and has at least one **solid** 4-neighbour (up, down, left or right;
a neighbour outside the image does not exist and cannot be solid) takes the
**average of the colours of its solid neighbours** — that is, the sum of the
neighbours' red, green and blue bytes divided by the number of solid
neighbours, with the division truncating toward zero — and stops being empty.
The neighbours' colours are the ones they hold **at that moment**, so a
neighbour that was itself given a colour in an earlier round donates that
colour.

A pixel filled during a round does **not** donate within the same round: a
neighbour becomes a donor only from the round after it was filled. Solid
pixels are donors from the start and never change.

The colour written into the receiving pixel is red = sum of the donors' red
divided by the donor count, green likewise, blue likewise (`utils.cpp:398-400`,
with the channel accumulation at `utils.cpp:321-323`).

**The alpha channel is never modified by this member.**

**Edge cases.**

- `level ≤ 0` → **raises** `"Invalid level count."` (`utils.cpp:343-344`).
- `threshold` is read as an 8-bit value first and then clamped into
  `[1, 255]` (`utils.cpp:345`, `:350-353`): the low 8 bits of the argument
  are taken, and a value of 0 becomes 1. A negative argument contributes its
  low 8 bits (so −1 becomes 255), and an argument of 256 becomes 0 and is
  then raised to 1. An argument of 300 becomes 44 (its low 8 bits) and is
  **not** clamped to 255.
- `fillColor` uses its low 24 bits as 0xRRGGBB (`utils.cpp:346-349`); a
  negative argument contributes its two's-complement low 24 bits.
- With the default `threshold = 1`, every pixel with alpha 0 is cleared to
  the fill colour and then stretched into; a fully opaque image is unchanged.
- A pixel at the image border has at most three neighbours.
- The fill colour is applied *before* the stretching, so a pixel that
  receives a donor colour no longer shows the fill colour.
- `level` rounds can leave pixels still empty when no solid pixel is within
  `level` steps; those keep the fill colour.

**Errors.** `"Invalid layer image."` when the image precondition fails
(`utils.cpp:358-359`); `"Invalid level count."` for `level ≤ 0`.

## 9. `Layer.isBlank`

**Signature.** `Layer.isBlank(x, y, w, h)` — an instance method on `Layer`,
four required integer arguments. Attached at `utils.cpp:519`; described at
`manual.tjs:126-130`.

**Result.** `true` when the region is blank, `false` otherwise.

**Semantics.** The region is clipped by the rule of §2.3. The region is
**blank** when every pixel in it has a **zero blue byte** (`utils.cpp:503`).
The check is on the blue channel alone: a pixel with a non-zero blue byte
makes the region non-blank, whatever its other channels are.

This is the upstream's actual test — it looks at the first byte of the pixel
(`utils.cpp:501-503`), which in the host's buffer is the blue channel
(§1). **A port on the engine's RGBA image must test the blue channel
(byte offset 2), not the byte at offset 0** (which is red there).

**Edge cases.**

- Fewer than four arguments → bad parameter count (`utils.cpp:474-476`).
- A region that is empty after clipping (width or height 0) → **true**
  (blank): the scan finds nothing.
- A region whose width or height is **negative** after clipping → **true**
  (blank): the scan is skipped entirely (`utils.cpp:498`).
- A region entirely outside the image (for example a positive `x` beyond the
  image width, which makes the clipped width negative) → **true**.
- The member never raises for an empty or out-of-range region.

**Errors.** `"src must be Layer."` when the image precondition fails
(`utils.cpp:481-483`).

## 10. `Layer.getAverageColor`

**Signature.** `Layer.getAverageColor(left, top, width, height)` — an
instance method on `Layer`, four required integer arguments. Attached at
`utils.cpp:618`; described at `manual.tjs:139-142`.

**Result.** An integer in `[0, 2^32)` — the packed average colour (§10.2).

**Semantics.** The region is clipped by the rule of §2.3. The four channels
are averaged **independently and unweighted** over the pixels of the clipped
region — every pixel counts once, alpha is not used as a weight
(`utils.cpp:595-603`). Each channel's mean is computed in floating point and
then **truncated toward zero** to an integer (`utils.cpp:604-611`): a mean of
127.9 becomes 127.

### 10.1 The averages

- `mean_blue` — the arithmetic mean of the blue byte over the region's
  pixels.
- `mean_green` — the mean of the green byte.
- `mean_red` — the mean of the red byte.
- `mean_alpha` — the mean of the alpha byte.

### 10.2 The packing — and the upstream's channel rotation

The returned word is assembled from the four means **in the order the bytes
appear in the host's buffer**, not in the order their names suggest
(`utils.cpp:598-611`): the byte at offset 0 of a pixel is summed into the
accumulator that ends up in the **top** byte of the result, the byte at
offset 1 into the next, the byte at offset 2 into the next, and the byte at
offset 3 into the **low** byte.

Because in the host's buffer offset 0 is blue, offset 1 is green, offset 2 is
red and offset 3 is alpha (§1), the returned integer is

`result = (mean_blue << 24) | (mean_green << 16) | (mean_red << 8) | mean_alpha`

that is, read as a 0xAARRGGBB colour word the result carries the **blue**
mean in the alpha field, the **green** mean in the red field, the **red**
mean in the green field and the **alpha** mean in the blue field. The
upstream's source names its accumulators as if the buffer were in
A, R, G, B order (`utils.cpp:588-591`), while the rest of the same plugin
(its PNG writer, its diff comparison, its ooze pass) treats the buffer as
B, G, R, A. The rotation is therefore an upstream inconsistency, not a
description of the channels.

**A port must reproduce the numbers, not the names.** The safest formulation
that is independent of the buffer's byte order is: *the returned word is the
four means of the four bytes of a pixel, packed so that the byte that comes
first in the buffer occupies the most significant byte of the word.* On the
engine's RGBA image that is `(mean_red << 24) | (mean_green << 16) |
(mean_blue << 8) | mean_alpha`; on the upstream's BGRA image it is the
rotation written above. The two differ, and the difference is observable.

### 10.3 Edge cases

- Fewer than four arguments → bad parameter count (`utils.cpp:561-563`).
- A region that is empty after clipping (width ≤ 0 or height ≤ 0) →
  **raises** `"invalid layer range"` (`utils.cpp:585-586`). Note the
  difference from `isBlank`, which answers `true` for the same region.
- A region that is empty because it lies outside the image also raises, since
  the clipping makes its width or height 0.
- The mean is over the clipped region, so a rectangle that hangs over the
  image edge averages only the part that is on the image.

**Errors.** `"src must be Layer."` when the image precondition fails
(`utils.cpp:568-570`); `"invalid layer range"` for an empty region.

## 11. `Layer.getFingerPrintValue`

**Signature.** `Layer.getFingerPrintValue(ignore_transparent = true)` — an
instance method on `Layer`.

- `ignore_transparent` (in, optional, default `true`) — whether a fully
  transparent pixel contributes its colour bytes to the hash.

Attached at `utils.cpp:674`; described at `manual.tjs:144-150`.

**Result.** A 64-bit hash, returned as a signed 64-bit integer: a hash whose
top bit is set comes back as a **negative** number (`utils.cpp:670`).

**Semantics.** An FNV-1a 64-bit hash (`utils.cpp:624-626`): the hash starts
at the basis `0xCBF29CE484222325`, and each byte is folded in as
`hash = (hash XOR byte) * 0x00000100000001B3` (all arithmetic modulo 2^64).

The bytes are fed in this order:

1. The pixels, row by row from the top, and within a row from the left. Each
   pixel contributes its four bytes **in buffer order** — blue, green, red,
   alpha on the upstream host (§1) — one after the other.
2. With `ignore_transparent` true (the default), a pixel whose **alpha byte
   is 0** contributes **four zero bytes** instead of its own bytes: its
   colour is ignored, not just its alpha (`utils.cpp:642-655`). With
   `ignore_transparent` false, every pixel contributes its own four bytes
   (`utils.cpp:656-662`).
3. The size, mixed in last (`utils.cpp:666-669`): for the shift values 24,
   16, 8 and 0 in that order, the byte `(width >> shift) & 255` is folded in
   and then the byte `(height >> shift) & 255` is folded in.

**Edge cases.**

- The argument is read as a TJS truth value: omitted or `void` means true; a
  numeric 0 means false; any other value means true (`utils.cpp:636`).
- Two images with the same pixels but different dimensions hash differently
  (the size is mixed in).
- The hash is **not** a collision-free identity: the upstream's own manual
  says a game must still compare exactly with `getDiffPixel` when the hash
  matches (`manual.tjs:148`).

**Errors.** `"Invalid layer image."` when the image precondition fails
(`utils.cpp:632-633`).

## 12. `Layer.getShrinkVectorOctet`

**Signature.** `Layer.getShrinkVectorOctet(w = 16, h = 16)` — an instance
method on `Layer`.

- `w` (in, optional, default `16`) — the target width in blocks.
- `h` (in, optional, default `16`) — the target height in blocks.

Attached at `utils.cpp:724`; described at `manual.tjs:152-157`.

**Result.** A TJS **octet** (binary) holding one 32-bit word per block, in
block order: blocks left to right within a row of blocks, rows of blocks top
to bottom (`utils.cpp:712-721`).

**Semantics.** The image is divided into a grid of blocks and each block is
reduced to one colour word — a cheap "thumbnail vector" of the image.

**The grid** (`utils.cpp:707-720`). The horizontal step is
`step_w = ceil(imageWidth / w)` and the vertical step is
`step_h = ceil(imageHeight / h)`. A block starts at every multiple of
`step_w` in x and of `step_h` in y that is still inside the image; the last
block of a row or column is cut short by the image edge. The number of blocks
per axis is therefore `ceil(imageWidth / step_w)` and
`ceil(imageHeight / step_h)`, which is **not** necessarily `w` and `h`: with
an image 100 pixels wide and `w = 16`, the step is 7 and there are 15
columns of blocks.

**The block value** (`utils.cpp:681-694`). Within a block, the four channel
bytes are summed over the block's pixels, but **only over the pixels whose
alpha byte is not 0**; the number of such pixels is the block's `total`. Each
channel's value is then `(sum + (total >> 1)) / total`, that is, the mean
rounded to the nearest integer with halves rounded up, truncated to 8 bits.
The four values are packed into one word with the **first buffer byte in the
low byte**: on the upstream host that is

`word = mean_blue | (mean_green << 8) | (mean_red << 16) | (mean_alpha << 24)`

i.e. an ordinary 0xAARRGGBB colour word (alpha in the top byte, blue in the
low byte).

The octet's bytes are the words' bytes in memory order, so the octet's byte
`4 * i` is the low byte (the blue mean) of block `i` (`utils.cpp:721`).

**Edge cases.**

- `w ≤ 0` or `h < 0` → **invalid parameter** (`utils.cpp:700`).
- `h == 0` passes the upstream's check and then divides by zero; the upstream
  has no defined behaviour there. **A port must not reproduce a crash**: it
  must raise the invalid-parameter error for `h ≤ 0`.
- A block that contains no pixel with a non-zero alpha has no defined value
  upstream (the same division by zero). **A port must produce a defined
  value**; zero is the natural one.
- The words are the means of the *non-transparent* pixels only, so a block
  that is half transparent averages the opaque half alone.
- The member returns an empty octet for an image whose block grid is empty —
  which cannot happen for a valid image, since the precondition requires a
  non-empty image.

**Errors.** `"Invalid layer image."` when the image precondition fails
(`utils.cpp:704-705`); invalid parameter for the sizes above.

## 13. `Math.octetVectorSum`

**Signature.** `Math.octetVectorSum(a, b)` — a **static** method on the
`Math` class. Attached at `utils.cpp:803`; described at `manual.tjs:163-172`.

- `a` (in, required) — an octet.
- `b` (in, required) — an octet, or `void` for the single-vector form.

**Result.** A TJS **array of seven integers**, indexed 0 to 6
(`utils.cpp:786-800`).

**The two-vector form** (`b` is an octet, `utils.cpp:755-775`). The two
octets must have the **same length**, otherwise the member raises the
invalid-parameter error. With `a_i` the i-th byte of `a` and `b_i` the i-th
byte of `b`:

| index | value |
|---|---|
| 0 | `Σ a_i · b_i` — the dot product |
| 1 | `Σ (a_i - b_i)²` — the squared Euclidean distance |
| 2 | `Σ a_i²` |
| 3 | `Σ b_i²` |
| 4 | `Σ a_i` |
| 5 | `Σ b_i` |
| 6 | the number of positions where `a_i ≠ b_i` |

(The squared distance uses the absolute difference, so the sign of
`a_i - b_i` does not matter.)

**The single-vector form** (`b` is `void`, or any non-octet value,
`utils.cpp:744-754`):

| index | value |
|---|---|
| 0 | 0 |
| 1 | `Σ a_i²` |
| 2 | `Σ a_i²` (the same value as index 1) |
| 3 | 0 |
| 4 | `Σ a_i` |
| 5 | 0 |
| 6 | the number of positions where `a_i ≠ 0` |

**Intended use** (from the upstream's manual, `manual.tjs:169`): index 0 is
the dot product, index 1 gives the Euclidean distance as `sqrt(r[1])`, and the
cosine similarity is `r[0] / (sqrt(r[2]) * sqrt(r[3]))`.

**Edge cases.**

- **Two arguments are required.** Calling the member with one argument raises
  the bad-parameter-count error (`utils.cpp:781`), even though the manual
  writes the second argument as optional (`manual.tjs:171`). To ask for the
  single-vector form, pass `void` explicitly as the second argument.
- `a` that is not an octet → invalid parameter (`utils.cpp:739`).
- `b` that is an octet of a different length → invalid parameter
  (`utils.cpp:758`).
- `b` that is not an octet and not `void` (a string, a number, an array) is
treated as "no second vector" — the single-vector form.
- The sums are computed in 64-bit signed integers and do not overflow for any
  realistic octet (a byte is at most 255, so `Σ a_i²` over a million bytes
  is about 6.5 · 10^10).
- An empty octet: the upstream may reject it depending on the host's empty
  octet handling; a port should accept it and answer all zeros.

**Errors.** Bad parameter count; invalid parameter (as above).

## 14. The `Window` save trio

The upstream attaches a native payload to the host `Window` class and three
methods to it (`Main.cpp:344-360`). The payload holds, per window, a table of
save slots (`Main.cpp:128`).

### 14.1 `Window.startSaveLayerImage(layer, filename, tags)`

**Signature.** `Window.startSaveLayerImage(layer, filename, tags)` — an
instance method on `Window`.

- `layer` (in) — the `Layer` to save.
- `filename` (in) — the file name to write.
- `tags` (in) — the tag dictionary for the file format, or `void`.

**Result.** An integer **handler** (`Main.cpp:274`).

**Semantics** (`Main.cpp:230-275`):

1. **Allocate a handler.** The window's slot table is searched for the first
   free slot; that slot's index is the handler. If every slot is busy, a new
   slot is appended and its index is the handler (`Main.cpp:231-240`). So the
   handlers are 0, 1, 2, … in allocation order, and a slot that becomes free
   is reused — the lowest free index wins.
2. **Copy the image.** A new `Layer` is created on the window, as a child of
   the window's `primaryLayer`, and named `"saveLayer:" + filename`. The
   source layer's image is copied into it with the host's own
   `Layer.assignImages` (`Main.cpp:242-270`). The copy is what gets saved,
   so the source layer may change while the save runs; the copy is also what
   the events report as their `layer` argument.
3. **Start the save.** The save runs on a detached thread (`Main.cpp:273`).
   The format is chosen by the file name's extension, lower-cased: `.png`
   means PNG, **anything else means TLG5** (`Main.cpp:324-331`).
4. **Return the handler.**

**Events.** The thread reports back to the window by posting a window message
that the window's own message receiver dispatches (`Main.cpp:156-173`); the
receiver calls two script methods on the window (`Main.cpp:80-90`,
`manual.tjs:31-47`):

- `onSaveLayerImageProgress(handler, progress, layer, filename)` — `progress`
  is a percentage (0 to 100). The event is sent only when the percentage
  **changes** (`Main.cpp:305`).
- `onSaveLayerImageDone(handler, canceled, layer, filename)` — `canceled` is
  1 when the save was cancelled, 0 otherwise.

Both events carry the handler, the **copied** layer and the file name. The
upstream does not define these methods; the game does. A window that does not
define them simply never sees the events (the upstream ignores a failed
call).

A progress or done event whose handler no longer belongs to that save is
dropped (`Main.cpp:136-151`): the done handler frees the slot **before** the
script callback runs, so a game may start a new save from inside the done
handler and the new save takes the same handler number.

**Errors.**

- The copy layer cannot be created → **`"保存処理用レイヤの生成に失敗しました"`**
  ("failed to create the layer for saving") (`Main.cpp:268`).
- The image cannot be copied → **`"保存処理用レイヤへの画像の複製に失敗しました"`**
  ("failed to copy the image into the layer for saving") (`Main.cpp:265`).
- The upstream performs **no argument-count check**: a missing file name is
  undefined behaviour upstream. A port must raise a typed error for a missing
  or non-string file name.
- The failure of the file write itself (the file cannot be opened) is
  reported by the encoders, which are out of scope (§15).

### 14.2 `Window.cancelSaveLayerImage(handler)`

**Signature.** `Window.cancelSaveLayerImage(handler)` — an instance method on
`Window`; `handler` (in) is the handler returned by
`startSaveLayerImage`.

**Semantics** (`Main.cpp:280-284`): the save identified by `handler` is
marked cancelled. The save thread notices at its next progress check, stops
writing, and **does not write the file**. The done event is still delivered,
with `canceled = 1` (`Main.cpp:86-90`, `:332-338`).

**Edge cases / errors.**

- A handler that is out of range, or a slot that is not busy → **nothing
  happens, silently** (`Main.cpp:281`).
- The member never raises.

### 14.3 `Window.stopSaveLayerImage(handler)`

**Signature.** `Window.stopSaveLayerImage(handler)` — an instance method on
`Window`; `handler` (in) is the handler returned by
`startSaveLayerImage`.

**Semantics** (`Main.cpp:289-294`): the save identified by `handler` is
stopped: the cancel flag is set, the window's notification link to it is
dropped and the slot is freed. **No done event is delivered** — the upstream's
own manual says so (`manual.tjs:26`), and the mechanism is that the
completion notification is never posted (`Main.cpp:114-117`, `:333-338`).

**Edge cases / errors.**

- A handler that is out of range, or a slot that is not busy → **nothing
  happens, silently** (`Main.cpp:290`).
- The member never raises.

### 14.4 What the port must decide

The handler table, the copy, the thread, the events, the cancel and the stop
are all in scope. The **encoder** the thread runs is not (§15): the port
must still allocate the handler, make the copy, deliver the progress and
done events and honour the cancel and stop, but what the file actually
contains is the out-of-scope task's business. See §17.

## 15. Out of scope: the encoders

The following members of the same upstream are **not** specified here and are
a separate task, because they need a file format the engine's tree does not
have (and, for TLG5, a committed fixture):

- `Layer.saveLayerImagePng(filename, tags)` — `savepng.cpp:538`, the PNG
  writer (`savepng.cpp:131`), using the bundled LodePNG unless the
  `LAYEREXSAVE_DISABLE_LODEPNG` build define is set.
- `Layer.saveLayerImagePngOctet(compression_level_or_tags)` —
  `savepng.cpp:555`, the same encoder returning the bytes instead of writing
  a file.
- `Layer.saveLayerImageTlg5(filename, tags)` — `savetlg5.cpp:278`, the TLG5
  writer (`savetlg5.cpp:186`).
- `Layer.saveProvinceImage(filename, palette)` — `savepng.cpp:575`, the
  province image as a 256-colour palette PNG; it raises when the layer has
  no province image (`savepng.cpp:508`).

Their only bearing on this specification is the extension rule of §14.1
(`.png` → PNG, otherwise TLG5) and the fact that the file is written only
when the save was not cancelled (`compress.hpp:161`, the save helper).

## 16. The upstream's build, its files, and what a port must settle

### 16.1 Build system

- **CMake** (`CMakeLists.txt`), built as a plugin of the KiriKiri Z SDK: the
  `krkrz_plugin()` macro comes from the `tp_stub` directory
  (`CMakeLists.txt:1-2`, the plugin block at `:6-20`), and the plugin's own
  sources are
  `LodePNG/lodepng.cpp`, `savepng.cpp`, `savetlg5.cpp`, `utils.cpp`,
  `Main.cpp`, `tlg5/slide.cpp` and the generated `LicensesGen.cpp`; it links
  `zlib` (`CMakeLists.txt:4`).
- The binding layer is **ncbind** (`ncbind.hpp`), which attaches members to
  the host's classes at load time; the plugin registers no class of its own.
- The upstream's readme tells the user to build the project with **premake4**;
  the tree in this clone carries the CMake build instead. The plugin can also
  be built with `LAYEREXSAVE_DISABLE_LODEPNG` defined, which replaces the
  LodePNG PNG path with the plugin's own simpler writer.
- The upstream is built against the **KiriKiri Z** host (`tp_stub`), which is
  the same host the engine's reference checkout is.

### 16.2 Where each member lives

| member | upstream file |
|---|---|
| `Layer.getCropRect` | `utils.cpp` (the member body at `:125-149`, attached at `:151`) |
| `Layer.getCropRectZero` | `utils.cpp` (`:169-193`, attached at `:195`) |
| `Layer.getDiffRect` | `utils.cpp` (`:222-256`, attached at `:258`) |
| `Layer.getDiffPixel` | `utils.cpp` (`:270-315`, attached at `:317`) |
| `Layer.oozeColor` | `utils.cpp` (`:337-419`, attached at `:421`) |
| `Layer.isBlank` | `utils.cpp` (`:471-517`, attached at `:519`) |
| `Layer.clearAlpha` | `utils.cpp` (`:525-550`, attached at `:552`) |
| `Layer.getAverageColor` | `utils.cpp` (`:558-616`, attached at `:618`) |
| `Layer.getFingerPrintValue` | `utils.cpp` (`:627-672`, attached at `:674`) |
| `Layer.getShrinkVectorOctet` | `utils.cpp` (`:695-723`, attached at `:724`) |
| `Math.octetVectorSum` | `utils.cpp` (`:778-802`, attached at `:803`) |
| `Window.startSaveLayerImage` | `Main.cpp` (`:230-275`, attached at `:356-360`) |
| `Window.cancelSaveLayerImage` | `Main.cpp` (`:280-284`, attached at `:356-360`) |
| `Window.stopSaveLayerImage` | `Main.cpp` (`:289-294`, attached at `:356-360`) |

### 16.3 What the port must decide for itself

1. **The DLL name and its aliases.** The upstream's build names the module
   `layerExSave`; the reference's plugin table lists `LayerExSave.dll`. The
   engine's built-in plugin table keys on the DLL name a game's
   `Plugins.link` call passes, and it already lists aliases for the
   case variants of other plugins — decide which spelling(s) this plugin
   answers to.
2. **The buffer order.** The upstream's members are byte arithmetic over a
   B, G, R, A buffer; the engine's layer image is R, G, B, A (§1). The
   mapping is mechanical, but `isBlank` (blue only), `getAverageColor`
   (the packing) and `getFingerPrintValue` / `getShrinkVectorOctet` (the byte
   order of the hash and of the words) are *order-sensitive* and their
   results change with the mapping. The port must pick the mapping that
   reproduces the upstream's observable numbers — see §17.1.
3. **The straight-alpha question.** The engine's layer image is
   straight-alpha; the upstream writes and compares the stored bytes without
   any alpha adjustment. A port must not premultiply or unpremultiply
   anywhere in these members.
4. **The premultiplied fill.** `clearAlpha` and `getDiffPixel` write a
   0xRRGGBB / 0xAARRGGBB word into the image verbatim (§6, §7). On a
   straight-alpha image that is the colour as given.
5. **The window message path.** The upstream delivers the events by posting
   a window message to the window's own `HWND` and letting the window's
   registered receiver dispatch them (`Main.cpp:156-173`). The engine has
   `Window.registerMessageReceiver` and a window-message path; the port must
   deliver the two script callbacks **on the engine's script thread**, never
   from a foreign thread.
6. **The thread.** The upstream saves on a detached thread. The engine's
   frame is owned by the engine (`crates/engine`), and the engine's rules
   forbid a background thread that touches engine state without a
   discipline; the port must decide how the copy is snapshotted and how the
   work is scheduled.
7. **The two division-by-zero cases** of §12 (`h == 0`, a block with no
   opaque pixel): the port must produce a defined value instead of a crash.

## 17. Open questions for the writer

1. **`getAverageColor`'s packing.** The upstream's returned word has the
   channels rotated relative to the channel they average (§10.2). Do we
   reproduce the upstream's numbers exactly — the word as the upstream builds
   it from *its* buffer order — or do we return the "sensible" 0xAARRGGBB
   average? The upstream is the oracle for the members, so the port should
   reproduce the numbers; but the port must be explicit about which buffer
   order it applies the formula to, because the two orders give different
   words. **Recommendation: reproduce the upstream's numbers** (the word
   built from the buffer bytes in buffer order), and say so in the test's
   citation.
2. **`isBlank`'s channel.** The upstream tests the blue byte (§9). On the
   engine's RGBA image the blue channel is byte 2. Reproducing "blue" is the
   faithful port; testing byte 0 would test red. **Recommendation: test the
   blue channel** and say so in the test.
3. **The hash and the shrink vector's byte order.** Both are order-sensitive
   (§11, §12). The port must fold the bytes in the upstream's order (blue,
   green, red, alpha) to reproduce the upstream's numbers, and must pack the
   shrink vector's words with the blue mean in the low byte.
4. **What `startSaveLayerImage` does without an encoder.** The handler, the
   copy and the events are specified; the file content is not. Does the port
   raise an error when the format is unsupported, or does it run the handler
   and event machinery and report the failure through the done event? The
   upstream reports a write failure by throwing inside the save thread (which
   the upstream does not catch), so the upstream has no defined behaviour
   there either.
5. **The province mask.** None of the members in this specification touch the
   province image; only the out-of-scope `saveProvinceImage` does. If the
   port wants the province for something else, that is a separate decision.
6. **`Math.octetVectorSum`'s array.** The result is an array of seven
   integers; the engine's VM must be able to build one and to read octets
   (it can — `crates/tjs/src/lib.rs` carries the octet variant and the
   `octet_bytes` accessor). Whether the port builds the array through the
   engine's own helper or by hand is the writer's call.
