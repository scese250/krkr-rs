# `layerExBTOA` — behavioural specification

A clean-room specification of the eight `Layer` members the upstream
`layerExBTOA` plugin adds to the host engine's `Layer` class. It is written
from the upstream source only; it contains no upstream code, no upstream
identifier except the plugin's own public entry points (the eight member
names), and no transliteration of the upstream control flow. Every claim
carries the upstream `file:line` that proves it, so a test can cite it.

## 0. Provenance and citation convention

* Upstream repository: `https://github.com/wtnbgo/layerExBTOA`
* Upstream commit read: `a57e7a25a9e079f95b93cf9768949e0f8789f685`
  ("getLayerClass をファイルローカル化 (静的リンク衝突回避)", Go Watanabe,
  2026-07-24).
* The plugin is one translation unit, `Main.cpp` (703 lines), plus two
  documentation files, `manual.tjs` (the author's pseudo-code manual) and
  `readme.txt` (usage notes), plus a sample `Movie.tjs` / `Movie.patch` that
  shows one real call site.
* Citation convention: `Main.cpp:146` means line 146 of `Main.cpp` at that
  commit; `manual.tjs:44` and `readme.txt:14` likewise. The upstream source
  is the oracle for these members: the engine's own reference checkout does
  not contain them, so a behaviour test cites the upstream file and line.
* The DLL name a script links is `layerExBTOA.dll` (the reference's plugin
  table, `reference/krkr2/docs/KRKR_PLUGINS.md:33`, names the upstream
  project for that DLL name).

## 1. How the members are registered

* The plugin defines **no class of its own**. At every call it resolves the
  host engine's global `Layer` class by evaluating the name `Layer` in the
  script global scope, and reads the members it needs off that class
  (`Main.cpp:15-20`, and every member's use of it, e.g. `Main.cpp:43`,
  `Main.cpp:88`, `Main.cpp:234`, `Main.cpp:366`, `Main.cpp:516`, `Main.cpp:568`,
  `Main.cpp:617`).
* The eight members are attached to that host `Layer` class as **native
  methods** (`Main.cpp:695-703`). The script-visible names are exactly:

  `copyRightBlueToLeftAlpha`, `copyBottomBlueToTopAlpha`, `fillAlpha`,
  `copyAlphaToProvince`, `clipAlphaRect`, `overwrapRect`, `fillByProvince`,
  `fillToProvince`.

  The author documents the same eight as methods of `Layer` in the manual
  (`manual.tjs:4-73`).
* No existing `Layer` member is replaced, renamed or shadowed; the plugin
  only adds these eight names to the class.
* Argument counts accepted (the upstream counts the parameters it was given,
  `Main.cpp:402`, `Main.cpp:527`, `Main.cpp:570`, `Main.cpp:619`; the
  zero-argument members simply ignore what they are given):

  | member | parameters |
  |---|---|
  | `copyRightBlueToLeftAlpha` | 0 |
  | `copyBottomBlueToTopAlpha` | 0 |
  | `fillAlpha` | 0 |
  | `copyAlphaToProvince` | 0 to 3 |
  | `clipAlphaRect` | 7 to 8 |
  | `overwrapRect` | 7 to 8 |
  | `fillByProvince` | 2 |
  | `fillToProvince` | 2 to 3 |

* Return values: every member returns nothing (`void`) except
  `fillToProvince`, which returns an array of four integers when it matched
  at least one pixel and nothing (`void`) when it matched none
  (`Main.cpp:674-689`).

### 1.1 How an optional argument is decided

The upstream's "does this argument exist" test is the TJS2 macro that is
*true only when the argument was passed and is not `void*`
(`crates/tjs/cpp/tjs2/tjsNative.h:492-493`, in the engine's vendored TJS2
header: the test is `numparams > n ? param[n]->Type() != tvtVoid : false`).
So:

* an argument that is **not passed** keeps the member's default;
* an argument that is passed **as `void`** also keeps the member's default
  (the manual says the same for `threshold`: "省略時またはvoidならそのまま
  コピーする", `manual.tjs:25`);
* an argument passed as any other value — including `0`, an empty string or
  a negative number — is used as given.

Argument values are converted to integers with the TJS2 integer conversion:
`void` → 0, an integer → itself, a real → truncated toward zero, a string →
parsed as an integer (an unparsable string → 0), an object or an octet →
a variant conversion error ("Cannot convert the variable type (%1 to %2)",
`crates/tjs/cpp/tjs2/tjsError.h:250-251`).

## 2. The data the members work on

### 2.1 The layer's own pixels (the "main image")

* The layer's image is a 32-bit-per-pixel buffer of rows of
  `mainImageBufferPitch` bytes; the image's size is `imageWidth` ×
  `imageHeight` (`Main.cpp:50-61`). The plugin reads the image size and the
  pitch, and the buffer address, through the host `Layer` members
  `hasImage`, `imageWidth`, `imageHeight`, `mainImageBufferPitch`,
  `mainImageBuffer` (read) and `mainImageBufferForWrite` (write)
  (`Main.cpp:50-61`, `Main.cpp:77-78`, `Main.cpp:254-255`, `Main.cpp:378`,
  `Main.cpp:381`, `Main.cpp:636-637`).
* A pixel is a 32-bit word whose value is `0xAARRGGBB`. In memory (the
  little-endian layout the upstream reads) the four bytes of a pixel are, in
  increasing address order: **blue, green, red, alpha** — the upstream
  writes and reads the alpha as the byte at offset `+3` inside the pixel
  (`Main.cpp:154`, `Main.cpp:190`, `Main.cpp:216`, `Main.cpp:272`,
  `Main.cpp:448`, `Main.cpp:467`) and reads the alpha/red/green/blue of a
  whole pixel as the word's bits 24-31 / 16-23 / 8-15 / 0-7
  (`Main.cpp:320-323`).
* The plugin never looks at the layer's *displayed* size (`width` / `height`)
  and never at the layer's position, and it ignores the layer's colour type
  except for one test (see 2.5).
* The alpha channel is a straight 8-bit alpha: it is copied, compared and
  multiplied as a plain 0-255 byte; nothing in the upstream premultiplies or
  unpremultiplies anything.

### 2.2 The province plane

* The province plane is a **one-byte-per-pixel** buffer of rows of
  `provinceImageBufferPitch` bytes (`Main.cpp:266-269`, `Main.cpp:589-592`).
* The upstream uses the byte purely as a value: it writes an index into it
  (`Main.cpp:662`), compares it for equality (`Main.cpp:600`) and copies the
  alpha byte into it (`Main.cpp:292`). It never interprets index 0 specially.
* The plane is read through `provinceImageBuffer` and written through
  `provinceImageBufferForWrite` (`Main.cpp:585-586`, `Main.cpp:261-262`,
  `Main.cpp:645-646`) — the "ForWrite" spelling is the host's way of
  obtaining a writable (and, in the host, freshly created) province plane.

### 2.3 The two coordinate spaces

* **Image coordinates** — the origin is the top-left pixel of the layer's
  image (`imageWidth` × `imageHeight`). The script's rectangle arguments
  (`dleft`, `dtop`) are in image coordinates (`Main.cpp:426-430`).
* **Clip coordinates** — the origin is the top-left pixel of the layer's clip
  rectangle; the clip rectangle is `clipLeft`, `clipTop`, `clipWidth`,
  `clipHeight` (`Main.cpp:98-105`). Members that work "inside the clip"
  address the image through the clip's origin and size.
* Which space a member uses:

  | member | region it touches |
  |---|---|
  | `copyRightBlueToLeftAlpha` | the whole image; the clip is **not** supported (`manual.tjs:8`, `Main.cpp:145`) |
  | `copyBottomBlueToTopAlpha` | the whole image; the clip is **not** supported (`manual.tjs:14`, `Main.cpp:181`) |
  | `fillAlpha` | the clip rectangle (`Main.cpp:213`) |
  | `copyAlphaToProvince` | the clip rectangle, for both the alpha source and the province destination (`Main.cpp:249`, `Main.cpp:258`, `Main.cpp:270`) |
  | `clipAlphaRect` | the destination's clip rectangle; the source's **whole image** (`Main.cpp:370`, `Main.cpp:373`) |
  | `overwrapRect` | the destination's clip rectangle; the source's **whole image** (`Main.cpp:370`, `Main.cpp:373`) |
  | `fillByProvince` | the clip rectangle, for both the province source and the pixel destination (`Main.cpp:577`, `Main.cpp:594`) |
  | `fillToProvince` | the clip rectangle, for both the pixel source and the province destination (`Main.cpp:631`, `Main.cpp:640`, `Main.cpp:654`) |

* The host's `clipLeft`/`clipTop`/`clipWidth`/`clipHeight` are the clip
  rectangle of the *layer*, and `clipWidth`/`clipHeight` are zero when the
  layer has no clip; the upstream treats a zero clip width or height as
  "nothing to do" and raises its "must be a Layer" error instead
  (`Main.cpp:112`).

### 2.4 The scaling arithmetic ("divide by 255")

Two places in the upstream scale a channel by an alpha factor. Both use the
same fixed-point approximation, on 32-bit unsigned integers:

    scale(c, a) = (c * a + ((c * a) >> 7)) >> 8

with `c` the channel value (0-255) and `a` the factor (0-255)
(`Main.cpp:450-451` for the alpha-only product, `Main.cpp:320-327` for the
per-channel product). Two consequences a test must know:

* it is **not** `round(c * a / 255)`: it can be one step below (for
  `c * a = 128` it yields `0` while `round(128 / 255) = 1`) or one step above
  (for `c * a = 33277` it yields `131` while `round(33277 / 255) = 130`).
  A port must use the formula, not a division by 255.
* for `a = 255` it is the identity (`scale(c, 255) = c` for every
  `c` in 0-255), and for `a = 0` it yields `0`.

A whole-pixel variant of the same scaling is used by `clipAlphaRect` on a
layer of type 12:

    mulPixel(p, a) =
      p                      if a == 255
      0x00000000             if a == 0
      pack(scale(A,a), scale(R,a), scale(G,a), scale(B,a))   otherwise

where `p` is the destination word `0xAARRGGBB` and the four channels are
scaled independently, the destination's own alpha included
(`Main.cpp:315-328`).

### 2.5 The layer type 12

The upstream compares the destination layer's `type` member with the
constant `12` (`Main.cpp:10`, `Main.cpp:422-423`); type 12 is the host
engine's "add alpha" layer type. Only `clipAlphaRect` reads it
(`Main.cpp:420-424`).

### 2.6 The error vocabulary

Four kinds of failure are distinguishable, and the script sees all of them
as an exception:

| kind | raised as | where |
|---|---|---|
| too few arguments | the TJS error `TJS_E_BADPARAMCOUNT`, rendered by the engine as **"Invalid argument count"** | `Main.cpp:402`, `Main.cpp:527`, `Main.cpp:570`, `Main.cpp:619` |
| a non-positive width or height | the TJS error `TJS_E_INVALIDPARAM`, rendered as **"Invalid argument"** | `Main.cpp:416`, `Main.cpp:540` |
| a precondition on the layer | a thrown message, listed per member below | `Main.cpp:146` and the other `TVPThrowExceptionMessage` sites |
| a non-object where a layer is expected | the variant conversion error "Cannot convert the variable type (%1 to Object)" | `Main.cpp:406`, `Main.cpp:531` |

The message texts the upstream throws are, verbatim:

* "dest must be Layer." — `Main.cpp:146`, `Main.cpp:182`, `Main.cpp:214`,
  `Main.cpp:371`
* "src must be Layer." — `Main.cpp:250`, `Main.cpp:374`, `Main.cpp:632`
* "src has no image." — `Main.cpp:256`, `Main.cpp:638`
* "dst has no province image." — `Main.cpp:263`, `Main.cpp:647`
* "dst has no province pitch." — `Main.cpp:268`, `Main.cpp:652`
* "must be Layer." — `Main.cpp:578`
* "no province image." — `Main.cpp:587`
* "no province pitch." — `Main.cpp:591`
* "Layer has no images." — `Main.cpp:384`

The three "is it a layer / does it have an image" preconditions are one
test each (`Main.cpp:46`, `Main.cpp:50`, `Main.cpp:64`, `Main.cpp:95`,
`Main.cpp:112`):

* the object is an instance of the host's `Layer` class;
* the layer reports `hasImage` non-zero;
* the width and the height it reports are positive and the pitch it reports
  is non-zero.

A layer that fails any of them produces the member's "must be a Layer"
message — the upstream does not distinguish "not a layer" from "no image"
from "empty image" in its message, and neither should the port.

### 2.7 Refreshing the layer

Six of the eight members ask the host to refresh the layer by calling its
`update` method with a rectangle `(left, top, width, height)`:

| member | update rectangle |
|---|---|
| `copyRightBlueToLeftAlpha` | `(0, 0, halfWidth, imageHeight)` — `Main.cpp:167` |
| `copyBottomBlueToTopAlpha` | `(0, 0, imageWidth, halfHeight)` — `Main.cpp:203` |
| `fillAlpha` | the clip rectangle — `Main.cpp:227` |
| `copyAlphaToProvince` | the clip rectangle — `Main.cpp:311` |
| `clipAlphaRect` | the clip rectangle when a clear value was given, otherwise the drawn rectangle — `Main.cpp:495-499` |
| `fillByProvince` | the clip rectangle — `Main.cpp:610` |
| `fillToProvince` | the clip rectangle — `Main.cpp:691` |

`overwrapRect` makes **no** refresh call at all: the member changes the
destination's pixels and returns (`Main.cpp:513-562` contains no update
call). This is an upstream omission, not a rule of the member; see §5.

## 3. The members

### 3.1 `Layer.copyRightBlueToLeftAlpha()`

**Signature.** A method of the host's `Layer` class, no arguments, returns
nothing (`Main.cpp:139-169`, `Main.cpp:695`).

**Purpose.** The layer's image is a video frame whose right half carries an
alpha channel in its blue bytes; the member moves that alpha into the left
half's alpha channel (`readme.txt:12-14`, `manual.tjs:6-10`).

**Preconditions and errors.** The layer must have an image with a positive
size and a non-zero pitch, and a writable main buffer; otherwise the
exception "dest must be Layer." is raised (`Main.cpp:145-147`, and the tests
behind it at `Main.cpp:64`, `Main.cpp:79`). Nothing else is checked: the
clip rectangle is ignored (`Main.cpp:145`; the manual says "clip not
supported", `manual.tjs:8`).

**Semantics.** Let `W = imageWidth`, `H = imageHeight` and
`half = W div 2` (integer division, `Main.cpp:150`). For every row
`y` in `0 .. H-1` and every column `x` in `0 .. half-1`:

* the **alpha byte** of the pixel at `(x, y)` becomes the **blue byte** of
  the pixel at `(x + half, y)` (`Main.cpp:153-165`: the source row starts at
  the pixel `half` columns to the right, the destination pointer is advanced
  by three bytes so that it addresses the alpha byte, and the copy advances
  four bytes per pixel).

Nothing else changes: the blue byte of the left half, the alpha byte of the
right half and every colour byte keep their values.

**Edge cases.**

* `W` odd: `half = (W-1) div 2`, so the source columns are
  `half .. half+half-1` and the **last column of the image is not read**;
  the destination covers columns `0 .. half-1` only.
* `W` or `H` zero, or a layer with no image: the "dest must be Layer."
  exception (§2.6).
* The refresh rectangle is `(0, 0, half, H)` (`Main.cpp:167`).

### 3.2 `Layer.copyBottomBlueToTopAlpha()`

**Signature.** A method of the host's `Layer` class, no arguments, returns
nothing (`Main.cpp:175-205`, `Main.cpp:696`).

**Purpose.** The same as 3.1 with the alpha channel in the **bottom** half
of the image (`readme.txt:41-43`, `manual.tjs:12-16`).

**Preconditions and errors.** As 3.1: the exception "dest must be Layer."
when the layer has no usable image (`Main.cpp:181-183`); the clip rectangle
is ignored (`manual.tjs:14`).

**Semantics.** Let `W = imageWidth`, `H = imageHeight` and
`half = H div 2` (`Main.cpp:186`). For every row `y` in `0 .. half-1` and
every column `x` in `0 .. W-1`:

* the **alpha byte** of the pixel at `(x, y)` becomes the **blue byte** of
  the pixel at `(x, y + half)` (`Main.cpp:189-201`).

The whole width is used; only the height is halved.

**Edge cases.**

* `H` odd: `half = (H-1) div 2`, so the source rows are
  `half .. half+half-1` and the **last row of the image is not read**.
* The refresh rectangle is `(0, 0, W, half)` (`Main.cpp:203`).

### 3.3 `Layer.fillAlpha()`

**Signature.** A method of the host's `Layer` class, no arguments, returns
nothing (`Main.cpp:207-229`, `Main.cpp:697`).

**Purpose.** Make the layer's alpha channel fully opaque over the clip
rectangle (`manual.tjs:18-21`).

**Preconditions and errors.** The layer must have an image, a non-empty clip
rectangle and a writable main buffer; otherwise the exception "dest must be
Layer." is raised (`Main.cpp:213-215`, with the tests at `Main.cpp:95`,
`Main.cpp:112`, `Main.cpp:79`).

**Semantics.** For every pixel of the clip rectangle (`clipLeft`, `clipTop`,
`clipWidth`, `clipHeight`), the **alpha byte** becomes `0xFF`; the red,
green and blue bytes are untouched (`Main.cpp:216-225`).

**Edge cases.**

* A layer with no image, or with an empty clip rectangle (clip width or
  height zero): the "dest must be Layer." exception. The member therefore
  never fills a zero-area region.
* The refresh rectangle is the clip rectangle (`Main.cpp:227`).

### 3.4 `Layer.copyAlphaToProvince(threshold, matched = 1, otherwise = 0)`

**Signature.** A method of the host's `Layer` class; `threshold`, `matched`
and `otherwise` are all optional (`Main.cpp:231-247`, `manual.tjs:23-30`).
The defaults are: `threshold` absent (or `void`) → "copy the alpha
verbatim"; `matched` absent → `1`; `otherwise` absent → `0`
(`Main.cpp:238`, `Main.cpp:239-247`). Returns nothing.

**Purpose.** Copy the layer's own alpha channel into the layer's own
province plane, optionally thresholded (`manual.tjs:23-30`).

**Preconditions and errors.** In this order:

1. the layer must have an image and a non-empty clip rectangle, else the
   exception "src must be Layer." (`Main.cpp:249-251`);
2. the layer's main buffer must be readable, else "src has no image."
   (`Main.cpp:253-257`);
3. the layer's province plane must be writable, else "dst has no province
   image." (`Main.cpp:260-264`);
4. the province pitch must be non-zero, else "dst has no province pitch."
   (`Main.cpp:265-269`).

**Semantics.** The clip rectangle is the region; for each pixel of it, the
alpha byte of the layer's main image (`Main.cpp:258`, `Main.cpp:272`) is
compared with `threshold` and the province byte of the same pixel
(`Main.cpp:270`) is written. Which rule applies depends on the three
arguments (`Main.cpp:276-285`):

* **`threshold` not given (or `void`)** — the province byte becomes the
  alpha byte verbatim (`Main.cpp:291-292`).
* **`threshold` in 0-255** — the province byte becomes `matched` where the
  alpha is **greater than or equal to** `threshold`, and `otherwise` where
  the alpha is below it (`Main.cpp:294-296`).
  * `matched` counts as "usable" when it is in 0-255, `otherwise` likewise;
    a value outside 0-255 means "do not write that side"
    (`Main.cpp:278-279`).
  * Both usable → every pixel of the region is written.
  * Only `matched` usable → pixels whose alpha is at least `threshold` are
    written with `matched`; the other pixels **keep the province value they
    had** (`Main.cpp:300-302`).
  * Only `otherwise` usable → pixels whose alpha is **below** `threshold`
    are written with `otherwise`; the other pixels keep their province
    value (`Main.cpp:303-305`).
  * Neither usable → **nothing is written at all**, the member returns
    normally and no refresh is requested (`Main.cpp:280`).
* **`threshold` at least 256** — every pixel of the region is written with
  `otherwise` (`Main.cpp:297-299`, the mode chosen at `Main.cpp:282-283`);
  `matched` is ignored. If `otherwise` is outside 0-255, nothing is written,
  the member returns normally and no refresh is requested
  (`Main.cpp:283-284`).
* **`threshold` below 0** — a negative number passed by the script takes the
  verbatim copy, exactly like "not given": the upstream tests the integer
  against 0 and 256 (`Main.cpp:277`, `Main.cpp:282`), so a value below 0
  takes neither branch and the member falls through to the verbatim copy
  (`Main.cpp:291-292`).

In one list: not given / `void` / negative → verbatim copy; 0 to 255 → the
thresholded rule; 256 or more → the "write `otherwise` everywhere" rule.

**Edge cases.**

* A layer with no image or an empty clip: "src must be Layer."
* A layer whose province plane cannot be obtained: "dst has no province
  image." / "dst has no province pitch."
* The comparison is an **unsigned byte** comparison: alpha 0-255 against
  threshold 0-255 (`Main.cpp:295`).
* The refresh rectangle is the clip rectangle (`Main.cpp:311`), and it is
  **not** requested in the two "nothing written" cases (`Main.cpp:280`,
  `Main.cpp:284`).

### 3.5 `Layer.clipAlphaRect(dleft, dtop, src, sleft, stop, swidth, sheight, clear)`

**Signature.** A method of the host's `Layer` class with seven required
arguments and one optional (`Main.cpp:389-415`, `manual.tjs:32-44`):

| # | name | direction | meaning |
|---|---|---|---|
| 0 | `dleft` | in | left edge of the destination rectangle, in the destination layer's **image** coordinates |
| 1 | `dtop` | in | top edge of the destination rectangle, in the destination layer's **image** coordinates |
| 2 | `src` | in | the source `Layer` object whose alpha channel multiplies the destination's |
| 3 | `sleft` | in | left edge of the source rectangle, in the **source layer's image** coordinates |
| 4 | `stop` | in | top edge of the source rectangle, in the source layer's image coordinates |
| 5 | `swidth` | in | width of the rectangle in pixels; must be > 0 |
| 6 | `sheight` | in | height of the rectangle in pixels; must be > 0 |
| 7 | `clear` | in (optional) | the value written over the part of the destination's clip rectangle that the rectangle does not cover; no default, and it is honoured only when it is a number in 0-255 |

The `clear` argument is honoured only when it was passed and is not `void`
and its integer value is in 0-255 (`Main.cpp:411-415`); a `clear` outside
0-255 (a negative number or 256 or more) means "do not clear"
(`manual.tjs:41-42`). Returns nothing.

**Purpose.** Multiply the destination's alpha channel by the source's alpha
channel over a rectangle (`manual.tjs:32-33`).

**Preconditions and errors.**

1. fewer than seven arguments → "Invalid argument count"
   (`Main.cpp:402`);
2. `swidth` or `sheight` is not positive → "Invalid argument"
   (`Main.cpp:416`); this is checked *before* the layers are inspected, so a
   bad size on a bad layer still reports "Invalid argument";
3. `src` is not an object → the variant conversion error
   "Cannot convert the variable type (%1 to Object)" (`Main.cpp:406`);
4. the destination layer must have an image, a non-empty clip rectangle and
   a writable buffer, else the exception "dest must be Layer."
   (`Main.cpp:370-372`);
5. the source layer must have an image with a positive size, else the
   exception "src must be Layer." (`Main.cpp:373-375`);
6. either buffer address being null → the exception "Layer has no images."
   (`Main.cpp:384`).

The destination is checked before the source (`Main.cpp:370` before
`Main.cpp:373`).

**Geometry.** Let the destination's clip rectangle be `(dl, dt, diw, dih)`
and the source's image size be `(siw, sih)`. The script's rectangle is
`[dleft, dleft+swidth) × [dtop, dtop+sheight)` in the destination's image
coordinates and `[sleft, sleft+swidth) × [stop, stop+sheight)` in the
source's image coordinates. The upstream reduces the destination origin to
clip-relative coordinates (`dx = dleft - dl`, `dy = dtop - dt`,
`Main.cpp:429-430`) and then intersects the two rectangles
(`Main.cpp:330-361`, called at `Main.cpp:433`):

* the source rectangle is clipped to the source image, and the destination
  origin is shifted by the same amount the source's left/top edge was cut
  by, so that the surviving source pixels stay paired with the destination
  pixels the script asked for (`Main.cpp:339-346`);
* the destination rectangle is clipped to the destination's clip rectangle,
  and the source origin is shifted by the same amount the destination's
  left/top edge was cut by (`Main.cpp:348-358`);
* if nothing survives on either axis, the member takes the "nothing to
  draw" path described below (`Main.cpp:360`, `Main.cpp:433`).

After the intersection, `w` and `h` are the surviving width and height, and
the surviving source pixel `(sleft', stop')` is paired with the surviving
destination pixel `(dleft', dtop')`.

**Semantics — destination type is not 12** (`Main.cpp:439-454`):

* Inside the surviving rectangle, the destination's **alpha byte** becomes
  `scale(dest_alpha, src_alpha)` (2.4) — the source's alpha byte is read at
  the source pixel's offset `+3` (`Main.cpp:448`). The colour bytes are
  untouched.
* When a `clear` value `c` was accepted, the alpha byte of every pixel of
  the destination's clip rectangle **outside** the surviving rectangle
  becomes `c` (`Main.cpp:440-445`, `Main.cpp:453`). "Outside" means: the
  rows above the rectangle and the rows below it, across the whole clip
  width, and, inside the rectangle's rows, the columns to its left and to
  its right. The colour bytes are untouched here too — only the alpha byte
  is cleared, even though the manual calls the argument an "opacity"
  (`manual.tjs:41`).

**Semantics — destination type is 12** (`Main.cpp:455-492`):

* Inside the surviving rectangle, the destination **pixel** becomes
  `mulPixel(dest_pixel, src_alpha)` (2.4): all four channels, the alpha
  included, are scaled by the source's alpha byte (`Main.cpp:466-468`,
  `Main.cpp:480-482`, `Main.cpp:488-490`).
* When a `clear` value `c` was accepted:
  * `c = 0` → every pixel of the destination's clip rectangle outside the
    surviving rectangle becomes `0x00000000`, all four bytes zeroed
    (`Main.cpp:459-470`);
  * `0 < c < 255` → every pixel of the destination's clip rectangle outside
    the surviving rectangle becomes `mulPixel(pixel, c)`, i.e. its four
    channels are scaled by `c` (`Main.cpp:471-484`);
  * `c = 255` → the outside pixels are left alone (scaling by 255 is the
    identity, so the upstream skips the work) (`Main.cpp:458`,
    `Main.cpp:486-492`).
* When no `clear` was accepted, the pixels outside the rectangle are left
  alone (`Main.cpp:486-492`).

**The "nothing to draw" path** (`Main.cpp:501-510`): when the rectangle does
not intersect the source image or the destination's clip rectangle at all:

* if a `clear` value was accepted, the **alpha byte** of every pixel of the
  destination's clip rectangle is set to that value — the colour bytes are
  *not* cleared, and this holds even for a type-12 destination where the
  `c = 0` case inside the rectangle zeroes the whole pixel
  (`Main.cpp:503-507`);
* otherwise nothing happens at all;
* in both cases the member returns normally (no exception).

**Refresh.** With a `clear` value the whole clip rectangle is refreshed;
without one, the surviving rectangle is refreshed, at the destination's
absolute image coordinates (`Main.cpp:495-499`).

**Edge cases.**

* A zero or negative `swidth`/`sheight`: "Invalid argument".
* A rectangle entirely outside the source image or the destination's clip:
  the "nothing to draw" path — no exception, and the destination is only
  cleared when a `clear` value was accepted.
* A partially outside rectangle: the rectangle is trimmed, never
  misaligned — the source pixel that pairs with a destination pixel stays
  the one the script asked for.
* A source layer with no image: "src must be Layer."; a source layer whose
  image is empty (zero width or height): also "src must be Layer."
  (`Main.cpp:64`).
* The source's own clip rectangle is ignored: the source's whole image is
  available (`Main.cpp:373`).
* If the destination layer's `type` member cannot be read, the upstream
  returns *success* without touching anything (`Main.cpp:422` returns a
  false value, which is the TJS success code); the member therefore does
  nothing and raises nothing in that case.

### 3.6 `Layer.overwrapRect(dleft, dtop, src, sleft, stop, swidth, sheight, threshold = 1)`

**Signature.** A method of the host's `Layer` class with seven required
arguments and one optional (`Main.cpp:513-539`, `manual.tjs:46-57`):

| # | name | direction | meaning |
|---|---|---|---|
| 0 | `dleft` | in | left edge of the destination rectangle, in the destination layer's **image** coordinates |
| 1 | `dtop` | in | top edge of the destination rectangle, in the destination layer's image coordinates |
| 2 | `src` | in | the source `Layer` object to lay over the destination |
| 3 | `sleft` | in | left edge of the source rectangle, in the **source layer's image** coordinates |
| 4 | `stop` | in | top edge of the source rectangle, in the source layer's image coordinates |
| 5 | `swidth` | in | width of the rectangle in pixels; must be > 0 |
| 6 | `sheight` | in | height of the rectangle in pixels; must be > 0 |
| 7 | `threshold` | in (optional, default `1`) | the smallest source alpha that is still laid over the destination |

`threshold` is used only when it was passed, is not `void` and its integer
value is in 0-255 (`Main.cpp:536-539`); a value outside 0-255 leaves the
default `1` in place (unlike `clear`, an out-of-range value does not
disable the feature). Returns nothing.

**Purpose.** Lay the source over the destination, skipping the source
pixels whose alpha is below the threshold — a workaround for `operateRect`
with an alpha mode (`manual.tjs:46-47`).

**Preconditions and errors.** Exactly as 3.5: "Invalid argument count"
(`Main.cpp:527`), "Invalid argument" for a non-positive size
(`Main.cpp:540`), the variant conversion error for a non-object `src`
(`Main.cpp:531`), "dest must be Layer." (`Main.cpp:371`), "src must be
Layer." (`Main.cpp:374`), "Layer has no images." (`Main.cpp:384`). The
destination is checked before the source.

**Geometry.** As 3.5: the same rectangle intersection, the same
clip-relative reduction of the destination origin (`Main.cpp:548-549`) and
the same trimming (`Main.cpp:552`, `Main.cpp:330-361`).

**Semantics.** For every pixel of the surviving rectangle, if the
**source pixel's alpha byte** is greater than or equal to `threshold`, the
destination pixel is **replaced by the source pixel in full** — all four
bytes, alpha included, copied verbatim (`Main.cpp:554-560`). Otherwise the
destination pixel is left alone. The comparison is an unsigned byte
comparison.

The source pixel is *not* blended: this is a copy, not a composite
(`Main.cpp:558`).

**Edge cases.**

* `threshold = 0` → every source pixel is laid over the destination,
  including the fully transparent ones.
* The default `threshold = 1` → a source pixel with alpha 0 is skipped, any
  other is laid over.
* A rectangle that does not intersect anything → the member returns
  normally and changes nothing (`Main.cpp:552`).
* **No refresh is requested** — the member never calls the layer's `update`
  (`Main.cpp:513-562`).

### 3.7 `Layer.fillByProvince(index, color)`

**Signature.** A method of the host's `Layer` class with two required
arguments (`Main.cpp:565-572`, `manual.tjs:59-64`):

| # | name | direction | meaning |
|---|---|---|---|
| 0 | `index` | in | the province index to look for; the low 8 bits of the argument's integer value |
| 1 | `color` | in | the colour to write; a 32-bit `0xAARRGGBB` word |

Returns nothing.

**Purpose.** Fill the layer's pixels with a colour wherever the province
plane carries a given index (`manual.tjs:59-60`).

**Preconditions and errors.**

1. fewer than two arguments → "Invalid argument count" (`Main.cpp:570`);
2. the layer must have an image, a non-empty clip rectangle and a writable
   buffer, else the exception "must be Layer." (`Main.cpp:577-579`);
3. the layer's province plane must be readable, else the exception "no
   province image." (`Main.cpp:585-588`);
4. the province pitch must be non-zero, else the exception "no province
   pitch." (`Main.cpp:589-592`).

**Semantics.** For every pixel of the clip rectangle, if the **province
byte** of that pixel equals `index`, the layer's pixel there is replaced by
the `color` word in full — alpha, red, green and blue (`Main.cpp:596-608`).
Pixels whose province byte is different keep their value.

**Argument conversion.**

* `index` is the argument's integer value truncated to 32 bits and then to
  8 bits (`Main.cpp:571`), i.e. its low 8 bits; a negative value wraps
  (e.g. `-1` → `255`).
* `color` is the argument's integer value truncated to 32 bits and read as
  an unsigned word (`Main.cpp:572`), i.e. `0xAARRGGBB` with the alpha in
  the high byte (`manual.tjs:62`). A negative argument becomes a word with
  the high bits set.

**Edge cases.**

* A layer with no province plane at all → "no province image." (this is the
  member's usual failure: the province plane only exists once something
  created it).
* A layer with an empty clip rectangle → "must be Layer."
* The refresh rectangle is the clip rectangle (`Main.cpp:610`).

### 3.8 `Layer.fillToProvince(color, index, threshold = 64)`

**Signature.** A method of the host's `Layer` class with two required
arguments and one optional (`Main.cpp:614-627`, `manual.tjs:66-72`):

| # | name | direction | meaning |
|---|---|---|---|
| 0 | `color` | in | the colour to look for; the argument's low 32 bits, of which the low 24 are the RGB to compare |
| 1 | `index` | in | the province index to write; the low 8 bits of the argument's integer value |
| 2 | `threshold` | in (optional, default `64`) | the smallest pixel alpha that still counts as a match |

Returns an array of four integers `[left, top, width, height]` — the
bounding box of the matched pixels — when at least one pixel matched, and
nothing (`void`) when none did (`Main.cpp:674-689`).

**Purpose.** Mark the pixels of a given colour in the province plane
(`manual.tjs:66-67`).

**Preconditions and errors.**

1. fewer than two arguments → "Invalid argument count" (`Main.cpp:619`);
2. the layer must have an image and a non-empty clip rectangle, else the
   exception "src must be Layer." (`Main.cpp:631-633`);
3. the layer's main buffer must be readable, else "src has no image."
   (`Main.cpp:635-639`);
4. the layer's province plane must be writable, else "dst has no province
   image." (`Main.cpp:644-648`);
5. the province pitch must be non-zero, else "dst has no province pitch."
   (`Main.cpp:649-653`).

**Semantics.** For every pixel of the clip rectangle, the layer's pixel is
read as a 32-bit word (`Main.cpp:658`) and the pixel is a **match** when
both hold (`Main.cpp:661`):

* the word's low 24 bits (the RGB) equal the `color` argument's low 24
  bits;
* the word's high byte (the alpha) is **greater than or equal to**
  `threshold` (the comparison is on the unsigned byte).

A matching pixel gets the province byte `index` (`Main.cpp:662`); a
non-matching pixel keeps the province value it had.

**Argument conversion.**

* `color` is the argument's 64-bit integer value truncated to 32 bits
  (`Main.cpp:620`), and the compared value is its low 24 bits
  (`Main.cpp:621`). The alpha of the argument is therefore ignored: the
  comparison is on RGB only, and the manual says the same
  (`manual.tjs:68`, "0xRRGGBB").
* **The upstream's "match every colour" mode is unreachable.** The upstream
  computes a flag that is meant to be true when the colour argument is
  negative, and tests it as "the 32-bit unsigned value is less than zero"
  (`Main.cpp:620-622`) — a comparison that is never true for an unsigned
  value. A negative colour argument therefore does **not** match every
  colour: `-1` is truncated to `0xFFFFFFFF`, whose low 24 bits are
  `0xFFFFFF`, so the member matches the pixels whose RGB is white.
* `index` is the argument's integer value truncated to 32 bits and then to
  8 bits (`Main.cpp:623`).
* `threshold` defaults to `64` (`Main.cpp:624`); it is used as given when
  it was passed and is not `void` (`Main.cpp:625-627`). It is compared
  against the pixel's alpha byte, so a value above 255 (or below 0) makes
  every pixel match, and a value of 0 makes every pixel match.

**The returned box.** The upstream tracks the smallest and largest matched
column and row, in clip-relative coordinates (`Main.cpp:656`, `Main.cpp:663-666`)
and reports them (`Main.cpp:674-689`):

* element 0 = the smallest matched column **plus `clipLeft`** — the
  absolute image coordinate of the box's left edge (`Main.cpp:680`);
* element 1 = the smallest matched row **plus `clipTop`** (`Main.cpp:681`);
* element 2 = the box's width in pixels = (largest matched column −
  smallest matched column + 1) (`Main.cpp:682`);
* element 3 = the box's height in pixels = (largest matched row − smallest
  matched row + 1) (`Main.cpp:683`).

When no pixel matched, the result is empty (`void`) and nothing is
returned (`Main.cpp:674-676`).

**Edge cases.**

* A layer with no image or an empty clip → "src must be Layer."
* No match → the member returns nothing, having changed nothing, and still
  requests the refresh of the clip rectangle (`Main.cpp:690-691`).
* The refresh rectangle is the clip rectangle (`Main.cpp:691`).

## 4. A note on the upstream's own documentation

The upstream's `manual.tjs` is the author's pseudo-code manual and it is
accurate about the argument names and the defaults, with two exceptions a
port must not copy:

* `manual.tjs:25` says a `void` threshold means "copy verbatim". That is
  true — but so is a *negative* threshold, because the upstream's branch
  tests the integer range and a negative number falls through to the
  verbatim copy (`Main.cpp:277`, `Main.cpp:282`, `Main.cpp:291-292`).
* The manual documents no "match every colour" mode for `fillToProvince`;
  the upstream source has the (unreachable) flag described in 3.8.

## 5. The upstream's build system, file layout, and the open port decisions

### 5.1 Build system and file layout

* The upstream is a **CMake** project (`CMakeLists.txt`) that builds one
  plugin target from a single source file, using a helper that is *not* in
  this repository: `CMakeLists.txt:1-2` points at a sibling `tp_stub`
  directory and includes `krkrz.cmake` from it, and the plugin target is
  declared with the sources `Main.cpp` (`CMakeLists.txt:3-8`).
* The plugin targets the **KiriKiriZ** ("krkrz") line of the engine and the
  **ncbind** C++ binding library: the source includes `ncbind.hpp`
  (`Main.cpp:1`) and attaches its members with ncbind's attach macros
  (`Main.cpp:695-703`).
* The upstream's comment at `Main.cpp:12-14` states that the helper that
  resolves the `Layer` class was made file-local because the plugin is
  **statically linked** into the host and would otherwise collide with a
  same-named global in another statically linked plugin (`layerExSave`).
  So the members are attached when the plugin's static library is linked
  into the host and its initialisers run; there is no runtime `Plugins.link`
  step inside the plugin itself. In the engine's own plugin system the
  equivalent is: the DLL name `layerExBTOA.dll` joins the built-in plugin
  table, and its members are installed the first time a script links that
  name.
* The member → file map is one-to-one — every member lives in `Main.cpp`:

  | member | lines in `Main.cpp` |
  |---|---|
  | `copyRightBlueToLeftAlpha` | 139-169 |
  | `copyBottomBlueToTopAlpha` | 175-205 |
  | `fillAlpha` | 207-229 |
  | `copyAlphaToProvince` | 231-313 |
  | `clipAlphaRect` | 389-511 |
  | `overwrapRect` | 513-562 |
  | `fillByProvince` | 565-612 |
  | `fillToProvince` | 614-693 |

  The shared helpers are in the same file: the layer-class lookup
  (15-20), the image-size and buffer helpers (40-80), the clip-size and
  buffer helpers (85-132), the pixel scaling helper (315-328), the
  rectangle-intersection helper (330-361) and the source/destination
  gatherer (362-387).

### 5.2 The things a port has to decide for itself

The specification above settles the behaviour; these are the decisions it
deliberately leaves open, because the upstream makes them through the host
engine's C++ API and the port has its own.

1. **How the pixels are reached.** The upstream takes raw addresses out of
   the host's `mainImageBuffer` / `mainImageBufferForWrite` /
   `provinceImageBuffer` / `provinceImageBufferForWrite` members and
   writes through them (`Main.cpp:77`, `Main.cpp:125`, `Main.cpp:254`,
   `Main.cpp:261`, `Main.cpp:378-382`, `Main.cpp:585-586`, `Main.cpp:636`,
   `Main.cpp:645`). A port may either use the same members (the engine
   already exposes all four) or work on the engine's own bitmap and
   province-plane types. The observable result must be the same: the
   destination's pixels change in place, and the province plane's bytes
   change in place.
2. **The "refresh" call.** Six members ask the host to refresh the layer
   (§2.7) and `overwrapRect` does not (§3.6). Whether the port's write
   path marks the layer dirty by itself, or whether it must reproduce the
   upstream's call pattern exactly (including `overwrapRect`'s omission),
   is a port decision — but a difference is script-visible when a script
   reads the layer back or when the layer is on screen, so the choice must
   be a deliberate one.
3. **The layer type 12 ("add alpha") pixels.** The upstream's type-12 path
   multiplies *all four* channels of the destination pixel, alpha included
   (2.4). Whether the engine's own "add alpha" representation stores those
   channels premultiplied or straight decides whether the port's arithmetic
   must be applied to the stored words as they are or to a converted form.
   The upstream's arithmetic is stated exactly in 2.4; the port must apply
   it to the words the engine stores.
4. **The province plane's allocation.** The upstream asks for the province
   plane through the "ForWrite" spelling in `copyAlphaToProvince` and
   `fillToProvince` (`Main.cpp:261`, `Main.cpp:645`) and through the
   read-only spelling in `fillByProvince` (`Main.cpp:585`). The engine's
   own province plane is created on demand; whether the "ForWrite" request
   creates it (so that the two "dst has no province image." failures are
   unreachable) or not is an engine decision that changes which of the
   upstream's error messages a script can ever see.
5. **The province index 0.** The upstream never treats the index 0
   specially (§2.2); the host engine's convention (0 meaning "no region")
   is not the upstream's business, and the port must not silently turn a
   written 0 into "no region" if the host's province plane uses 0 that way.
6. **The error messages.** The upstream's message texts (§2.6) are
   script-visible: a script may catch the exception and read its `message`.
   The port should raise the same texts, or record a deliberate divergence.
7. **The arity errors.** "Invalid argument count" and "Invalid argument"
   are the host's spellings of the two TJS arity/parameter errors
   (`Main.cpp:402`, `Main.cpp:416`); the port should raise the same two
   conditions with the same wording so that a script's `try/catch` sees the
   same thing.
8. **The `void` argument.** The upstream's "an argument that was passed as
   `void` counts as not passed" rule (§1.1) is a TJS2 semantic, and the
   port's native-call bridge must reproduce it for the four members with
   optional arguments (`copyAlphaToProvince`'s three arguments,
   `clipAlphaRect`'s `clear`, `overwrapRect`'s `threshold` and
   `fillToProvince`'s `threshold`).

## 6. What this specification does not cover

* The `Movie.tjs` / `Movie.patch` sample is a *consumer* of the plugin, not
  part of it; it shows the intended call pattern — halve the layer's
  display width to the image width divided by two, then call
  `copyRightBlueToLeftAlpha` on every frame (`Movie.patch:51-55`, and the
  frame hook at `Movie.patch:33-50`; the same pattern is in `readme.txt:19-33`).
  It also shows the DLL name a script links, `layerExBTOA.dll`
  (`Movie.patch:75`). The engine's own `Movie` implementation is a separate
  matter.
* The plugin has no state, no configuration, no file format and no fixture:
  every member's whole behaviour is in the one upstream function that
  implements it. There is nothing to fixture and nothing to load.
