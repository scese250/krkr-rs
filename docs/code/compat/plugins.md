# Native plugins (`compat`)

The native plugins are the emulations of the KiriKiri plugin DLLs a game
names in `Plugins.link("…dll")`. They live in the compatibility layer
(`crates/compat`), are **statically built into the binary**, and are the only
place where a plugin's script-visible surface is registered.

* Source: `crates/compat/src/natives/plugins/`
* Registry: `crates/compat/src/plugin.rs` (`PluginRegistry`, `BuiltinPluginDef`)
* Script-facing class: `crates/compat/src/plugins_native.rs` (`Plugins`)
* Parity report: [`docs/plugin_parity.md`](../../plugin_parity.md)
* Inventory: the [built-in table](#the-built-in-table) below, and the plugins the engine has not built in, [further down](#the-plugins-the-engine-has-not-built-in)
* Design background: [`docs.old/migration/08-tjs-vm-and-native-plugins.md`](../../../docs.old/migration/08-tjs-vm-and-native-plugins.md)

## How a plugin is linked

`Plugins` exposes three static members:

| member | returns | behaviour |
|---|---|---|
| `Plugins.link(name)` | `int` | activates the built-in emulation for `name` and answers `1`, or warns and answers `0` when the name is not ported |
| `Plugins.unlink(name)` | `bool` | changes nothing and answers `1` for every name |
| `Plugins.getList()` | `Array` | the linked modules, lower-cased and sorted by name |

`link` never throws for an unknown DLL. A name that resolves to a ported
plugin resolves to the built-in table; any other name is **absent**: one
warning names the plugin, nothing is registered for it — no script global
under the DLL's stem and no object that accepts every member read and every
call — and nothing is loaded, so `Plugins.getList` does not name it. The
reference joins a name to the set the list reports only when the module was
loaded: the name joins it after the module's registrations have run, and a
name that matches no module returns without joining it
(`reference/cpp/core/plugin/ncbind.cpp:12-28`), so the list holds the
modules that were loaded (`reference/cpp/core/plugin/PluginImpl.cpp:332-349`).
A game that probes for the class (`typeof global[name] == 'Object'`)
therefore sees it missing and takes its own "class not found" path, exactly
as it does against an engine that could not load the DLL.

A `.tpm` name and a `.dll` name are one module: the extension is rewritten
before the lookup, so `Plugins.link("foo.tpm")` names the module
`Plugins.link("foo.dll")` names, the way the reference rewrites it before it
loads a module (`reference/cpp/core/plugin/PluginImpl.cpp:95-146`).

A `.tpm` file in the executable directory, `<exe>/system` or `<exe>/plugin` is
auto-loaded at startup: the trailing `.tpm` becomes `.dll` and the name is
linked the same way, so `extrans.tpm` resolves to the same built-in as
`Plugins.link("extrans.dll")`.

Only the startup module (`xp3filter.dll`) is registered before the startup
scripts run; every other built-in registers its members on the first
`Plugins.link` that names it, the same point at which the reference runs a
module's `V2Link` registration. The timing is script-observable: a shipped
`system/voice.tjs` probes `typeof WaveSoundBuffer.setDefaultAheads` before it
links `getSample.dll` and only then publishes its own
`WaveSoundBuffer.enableGetSample` flag. Registration is idempotent: linking a
built-in whose classes are already installed records the link without
registering again.

A plugin may declare a startup dependency on another module. `emoteplayer.dll`
loads `motionplayer.dll` first, and `getabout.dll` loads `windowEx.dll`
because the `System.getAboutString` member lives in that module's extension;
the dependency is registered on the same link and is named by
`Plugins.getList`, because the dependency load runs through the same module
load as the plugin's own — the reference's pre-registration callback, whose
module lands in the same set the list reports
(`reference/cpp/core/plugin/ncbind.cpp:12-28`).

## The built-in table

`crates/compat/src/natives/plugins/mod.rs` holds one `BuiltinPluginDef` per
DLL name. The table is also the source of the parity report, which compares
the reference plugin surface (`reference/cpp/plugins/**`) against the classes
the port registers.

| DLL name | aliases | Rust module | classes / globals it installs | page |
|---|---|---|---|---|
| `addFont.dll` | — | `add_font` | `System.addFont` (static) | [add_font](plugins/add_font.md) |
| `csvParser.dll` | — | `csv_parser` | `CSVParser` | [csv_parser](plugins/csv_parser.md) |
| `dirlist.dll` | — | `dirlist` | global `getDirList` | [dirlist](plugins/dirlist.md) |
| `emoteplayer.dll` | — | `emote_player` | none (loader shim for `motionplayer.dll`) | [emote_player](plugins/emote_player.md) |
| `extrans.dll` | — | `extrans` | none (the transition providers are dispatched by `Layer.beginTransition`) | [extrans](plugins/extrans.md) |
| `fftgraph.dll` | — | `fft_graph` | global `drawFFTGraph` | [fft_graph](plugins/fft_graph.md) |
| `fstat.dll` | — | `fstat` | `Storages.*` stat members, `TemporaryFiles` | [fstat](plugins/fstat.md) |
| `getabout.dll` | — | `get_about` | none (`System.getAboutString` is owned by `windowEx`) | [get_about](plugins/get_about.md) |
| `getSample.dll` | — | `get_sample` | members on `WaveSoundBuffer` (`getSample`, `sampleValue`, `sampleCount`, `sampleAhead`, the defaults setters) | [get_sample](plugins/get_sample.md) |
| `json.dll` | — | `json` | `Scripts.evalJSON`, `evalJSONStorage`, `saveJSON`, `toJSONString` | [json](plugins/json.md) |
| `KAGParserEx.dll` | `KAGParser.dll` | `natives::kag` (core) | none — `KAGParser` is a core native registered at startup | — (core native, not a plugin module) |
| `layerExDraw.dll` | `layerExMovie.dll`, `perspective.dll` | `layer_ex` | the `GdiPlus` namespace, `Layer.draw*`, `Layer.perspectiveCopy`, the movie members | [layer_ex](plugins/layer_ex.md) |
| `menu.dll` | — | `natives::menu_item` | `MenuItem` | — (core native, not a plugin module) |
| `motionplayer.dll` | — | `motion_player` | `Motion`, `Player`, the `Motion.*` subclasses, the emote classes | [motion_player](plugins/motion_player.md) |
| `psbfile.dll` | — | `psb_file` | `PSBFile` | [psb_file](plugins/psb_file.md) |
| `psd.dll` | — | `psd_file` | `PSD` | [psd_file](plugins/psd_file.md) |
| `saveStruct.dll` | — | `save_struct` | `Array.save2`/`saveStruct2`/`toStructString`, `Dictionary.saveStruct2`/`toStructString` | [save_struct](plugins/save_struct.md) |
| `ScriptsEx.dll` | — | `scripts_ex` | the `Scripts.*` helpers, `Scripts.rehash` | [scripts_ex](plugins/scripts_ex.md) |
| `shrinkCopy.dll` | — | `shrink_copy` | `Layer.shrinkCopy`, `Layer.shrinkCopyFast` | [shrink_copy](plugins/shrink_copy.md) |
| `krkrsteam.dll` | `steam_api.dll`, `DrawDeviceForSteam.dll` | `steam` | `Steam`, `steam_api`, `DrawDeviceForSteam` | [steam](plugins/steam.md) |
| `TextRender.dll` | — | `text_render` | `TextRenderBase` | [text_render](plugins/text_render.md) |
| `varfile.dll` | — | `var_file` | the `var://` storage media | [var_file](plugins/var_file.md) |
| `win32dialog.dll` | — | `win32_dialog` | `WIN32Dialog` | [win32_dialog](plugins/win32_dialog.md) |
| `windowEx.dll` | — | `window_ex` | members on `System`, `Scripts`, `Window`, `MenuItem`, `Console` and `Pad` | [window_ex](plugins/window_ex.md) |
| `wutcwf.dll` | — | `wutcwf` | none (it installs a `TCWF` wave decoder into the audio chain) | [wutcwf](plugins/wutcwf.md) |
| `wuvorbis.dll` | — | `wuvorbis` | none (the decoder lives in the audio chain) | [wuvorbis](plugins/wuvorbis.md) |
| `xp3filter.dll` | — | `xp3_filter` | `Storages.setXP3ArchiveExtractionFilter`, `Storages.setXP3ArchiveContentFilter` (deferred half) | [xp3_filter](plugins/xp3_filter.md) |

`simplebinder` is not a plugin DLL: the reference ships it as a header-only
C++ helper other plugins include, so the Rust module
(`crates/compat/src/natives/plugins/simple_binder/`) installs nothing. Its page
is [simple_binder](plugins/simple_binder.md).

## The plugins the engine has not built in

The table above holds the plugins the engine builds in. This section holds
the ones it does not, the members of the built-in plugins that are still
missing, and what each of them would take.

**`shrinkCopy` is built in.** Its two members — `Layer.shrinkCopy` and
`Layer.shrinkCopyFast` — are the only ones the plugin registers, so it is not
in the lists below.

The engine builds in **27** plugin names. The upstream organisation's trees
carry 46 plugins and two transition providers; 15 of the plugins and one of
the transition providers are built in, so **31 plugins and one transition
provider** are not, and seven built-in plugins still lack some of their
members.

### What a plugin the engine has not built in means at run time

A plugin the engine has no emulation for **registers nothing**.
`Plugins.link` answers `0`, one warning names the plugin, and the script
sees the plugin as absent: no class under the DLL's stem, no global, no
member. Nothing is loaded, so `Plugins.getList` does not name it either. A
game that probes for the class takes its own "class not found" path, exactly
as it does against an engine that could not load the DLL.

A member of a built-in plugin that the engine does not have is missing the
same way: the plugin links and answers `1`, but the name is not on the class,
so the call raises. Every member listed below is therefore unreachable from a
script until it is implemented.

### Where the behaviour comes from

The plugins in the tables below are the ones the `wamsoft` organisation's trees
carry, each compared member by member with the surface the engine registers.
The trees carry **no binaries**: every one of them publishes its source, so
the upstream source is the oracle for every member below, and the upstream
project of each name is named in the reference's own plugin table where it
has one. Where a game's shipped build is the oracle instead of the upstream
source, the citation is the build's file name and the sha256[:12] of that
build, and a committed fixture carries the bytes — the build itself is never
loaded at test time.

The tables are grouped by what implementing them needs. A row names a plugin
the engine has **not** built in unless it says *built in*: a built-in plugin
that lacks a few members is marked, and only the members in that row are
missing.

### One upstream function

Each member's whole behaviour is in one upstream function: no new format, no
fixture.

| plugin | what it would provide | what an implementation needs |
|---|---|---|
| `layerExAreaAverage` | `Layer.stretchCopyAA` — a stretch copy that averages the source pixels | the upstream source (`wtnbgo/layerExAreaAverage`) |
| `layerExRaster` | `Layer.copyRaster` | the upstream source (`wtnbgo/layerExRaster`) |
| `layerExImage` | `Layer.colorize`, `Layer.modulate`, `Layer.noise` and `Layer.generateWhiteNoise` — four per-pixel colour operations | the upstream source (`wtnbgo/layerExImage`) |
| `layerExLongExposure` | `Layer.initExposure`, `Layer.snapExposure`, `Layer.statExposure`, `Layer.copyExposure` and `Layer.termExposure` — an exposure accumulator | the upstream source (`wamsoft/layerExLongExposure`) |
| `layerExBTOA` | eight `Layer` members that move the alpha channel between the image and the province: `copyRightBlueToLeftAlpha`, `copyBottomBlueToTopAlpha`, `fillAlpha`, `copyAlphaToProvince`, `clipAlphaRect`, `overwrapRect`, `fillByProvince` and `fillToProvince` | the upstream source (`wtnbgo/layerExBTOA`) |
| `layerExSave` | the analytic members — `Layer.clearAlpha`, `Layer.oozeColor`, `Layer.getAverageColor`, `Layer.getCropRect`, `Layer.getCropRectZero`, `Layer.getDiffPixel`, `Layer.getDiffRect`, `Layer.getFingerPrintValue`, `Layer.getShrinkVectorOctet`, `Layer.isBlank`, `Math.octetVectorSum`, and `Window.startSaveLayerImage`, `Window.stopSaveLayerImage` and `Window.cancelSaveLayerImage` | the upstream source (`wtnbgo/layerExSave`) |
| `saveStruct` *(built in)* | `Scripts.toStructString` | the upstream source (`wtnbgo/saveStruct`) |
| `scriptsEx` *(built in)* | `Scripts.stringFuzzySearch` | the upstream source (`wtnbgo/scriptsEx`) |
| `KAGParserEx` *(built in)* | `KAGParser.hasLabel` — whether a scenario holds a label | the upstream source (`wtnbgo/KAGParserEx`) |
| `layerExDraw` *(built in)* | the sixteen `Layer.EncoderValue*` constants and `Layer.getColorRegionRects` | the upstream source (`wtnbgo/layerExDraw`) |
| `layerExVector` | `GdiPlus.DashStyleCustom` and the four font properties the engine's `GdiPlus.Font` does not carry: `fontSize`, `italic`, `letterSpacing` and `fontFamily` | the upstream source (`wamsoft/layerExVector`) |

A member of this shape lands as a thin member over the engine's own layer,
the DLL name joins the built-in table, the registered surface changes and its
baseline is updated with it, and the behaviour test cites the upstream file
and line.

These members are one upstream function as well, but what they do belongs to
the host or to one of the engine's own services, so they need a host seam
beside the source.

| plugin | what it would provide | what an implementation needs |
|---|---|---|
| `process` | `System.commandExecute` and the `Process` class (`commandExecute`, `execute`) | the upstream source (`wamsoft/process`); a process is the host's work, so the member is a thin call over a host seam |
| `shellExecute` | `System.commandExecute` | the upstream source (`wamsoft/shellExecute`); the same host seam |
| `systemEx` | 26 `System` members and constants: `writeRegValue`, `writeEnvValue`, `readEnvValue`, `expandEnvString`, `urlencode`, `urldecode`, `confirm`, `getKnownFolderPath`, `getOSVersion`, `setDpiAwareness` and the rest | the upstream source (`wamsoft/systemEx`); the registry and the environment are the host's |
| `windowEx` *(built in)* | the 12 members the engine does not have: `System.classLongPtr`, `findWindowEx`, `loadCursor`, `mapVirtualKey`, `setClipCursor`, `setDpiAwareness`, and `Window.acquireImeControl`, `resetImeContext`, `maximizeBox`, `minimizeBox`, `resetExSystemMenu` and `setWindowCornerPreference` | the upstream source (`wtnbgo/windowEx`); the cursor and the window message are the platform's |
| `steam` *(built in)* | `Steam.RestartAppIfNecessary` and `Steam.isDlcInstalled` | the upstream source (`wamsoft/steam`); the Steam client is a platform service |
| `layerExVector` | the vector text layout: `GdiPlus.loadFont`, `GdiPlus.unloadFont` and `Layer.drawStringArea` | the upstream source (`wamsoft/layerExVector`); the layout runs in the engine's text service |
| `win32dialog` *(built in)* | the dialog: the 1 032 `WIN32Dialog` constants, the 74 members the dialog itself still lacks, and the helper classes `Blob`, `Bitmap`, `DrawItem`, `Header`, `Items`, `NotifyAccessor` and `SolidBrush` with their 30 members | the upstream source (`wtnbgo/win32dialog`); a dialog is the platform's |
| `stdio` | the console handles: `System.stdin`, `System.stdout`, `System.stderr`, `System.stdioState`, `System.allocConsole`, `System.attachConsole`, `System.freeConsole` and `System.flush` | the upstream source (`wamsoft/stdio`); the console is the host's |

### A format and a fixture

The member needs a format the tree does not have, and the bytes of that
format are not reproducible from the tree. The upstream source gives the
algorithm; a fixture committed under `crates/compat/tests/fixtures/` carries
the bytes, and the test reads the fixture — the plugin's own build is never
loaded at test time.

| plugin | what it would provide | what an implementation needs |
|---|---|---|
| `layerExSave` | the PNG and TLG5 encoders — `Layer.saveLayerImagePng`, `Layer.saveLayerImagePngOctet`, `Layer.saveLayerImageTlg5` and `Layer.saveProvinceImage` | the upstream source (`wtnbgo/layerExSave`) and a fixture for the encoded bytes: TLG5 exists nowhere else in the tree |
| `layerExDraw` *(built in)* | the image encoder `Layer.saveImage` | the upstream source (`wtnbgo/layerExDraw`) and a fixture for the encoded image |
| `tftSave` | the pre-rendered-font container — `System.savePreRenderedFont`, `System.loadPreRenderedFont`, `System.modifyPreRenderedFont`, `Layer.renderGlyph`, `Layer.setGlyphInfo` and `Layer.glyphCharset` | the upstream source (`wamsoft/tftSave`) and a fixture for the container |
| `layerExAVI` | the AVI container and the WAV writer — `Layer.openAVI`, `Layer.openCompressedAVI`, `Layer.recordAVI`, `Layer.closeAVI`, `Layer.openWAV`, `Layer.startWAV`, `Layer.stopWAV` and `Layer.closeWAV` | the upstream source (`wamsoft/layerExAVI`) and a fixture for the container |
| `minizip` | the zip reader and writer — `Storages.mountZip`, `Storages.unmountZip` and the `Zip` and `Unzip` classes | the upstream source (`wtnbgo/minizip`) and a fixture for the archive |
| `sigcheck` | the signature check — `Window.checkSignature`, `Window.stopCheckSignature` and `Window.cancelCheckSignature` | the upstream source (`wamsoft/sigcheck`) and a signed fixture |
| `memfile` | the memory storage medium — the seven `Storages` members `isExistMemoryFile`, `getMemoryFileInfo`, `getMemoryFileData`, `deleteMemoryFile`, `isExistMemoryDirectory`, `getMemoryDirectory` and `deleteMemoryDirectory` | the upstream source (`wamsoft/memfile`) and a fixture for the stored bytes |

### A whole subsystem

Each is a subsystem with its own shape, and the service it belongs to has to
be decided before its first member: a rich-text layout is the engine's text
service, a controller is the platform's input, a transition is the engine's
own, and a process is the host's.

| plugin | what it would provide | what an implementation needs |
|---|---|---|
| `krkr_richtext` | a rich-text layout engine — seven classes and 57 members | the upstream source (`wamsoft/krkr_richtext`) |
| `krkrthreepp` | a three.js and VRM runtime — 93 members | the upstream source (`wamsoft/krkrthreepp`) |
| `krkrlive2d` | a Live2D runtime — four classes and 21 members | the upstream source (`wamsoft/krkrlive2d`) |
| `gamepad` | a controller stack — two classes and 39 members | the upstream source (`wamsoft/gamepad`) |
| `clipfile` | a CLIP STUDIO PAINT reader and writer — two classes and 38 members | the upstream source (`wamsoft/clipfile`) |
| `win32ole` | COM, OLE and `ActiveX` — three classes and 27 members | the upstream source (`wamsoft/win32ole`) |
| `binaryStream` | a compressed binary stream — 27 members | the upstream source (`wamsoft/binaryStream`) |
| `httprequest` | an HTTP client — 17 members | the upstream source (`wamsoft/httprequest`); the network is the host's |
| `httpserv` | an HTTP server — 5 members | the upstream source (`wamsoft/httpserv`); the same host seam |
| `expat` | an XML parser — the `XMLParser` class | the upstream source (`wamsoft/expat`) |
| `lineParser` | a line parser — the `LineParser` class, the shape the engine's `CSVParser` already has | the upstream source (`wamsoft/lineParser`) |
| `AlphaMovie` | an alpha-movie player — 19 members | the upstream source (`wamsoft/AlphaMovie`); a movie is a media service |
| `messenger` | the user messages between windows — 9 members | the upstream source (`wamsoft/messenger`) |
| `windowExProgress` | a progress bar on `Window` — 5 members | the upstream source (`wamsoft/windowExProgress`) |
| `resourceRW` | a reader and a writer for a program's own resources — two classes and 49 members | the upstream source (`wamsoft/resourceRW`); it edits the built program, not a game's data |

### The transition providers and the host hooks

Two of the organisation's plugins register transition handlers instead of a
script class, and three hook the host without a script surface of their own.

| plugin | what it would provide | what an implementation needs |
|---|---|---|
| `extrans` | seven transitions — `wave`, `mosaic`, `turn`, `rotatezoom`, `rotatevanish`, `rotateswap` and `ripple` | built in: the engine's own transition module answers all seven |
| `extNagano` | twelve more transitions — `3duniversal`, `blurfade`, `book`, `flutter`, `honeyturn`, `imagewipe`, `morphing`, `multiripple`, `rgbfade`, `scanline`, `spin` and `zoomfade` | the upstream source (`wamsoft/extNagano`); a transition belongs to the engine's own transition module, beside the seven above |
| `console` | attaches a console to the process | the upstream source (`wamsoft/console`); the console is the host's, and the plugin installs no script surface of its own |
| `fpslimit` | caps the frame rate | the upstream source (`wamsoft/fpslimit`); the same host seam, and no script surface of its own |
| `msgreceiver` | receives the host's window messages | the upstream source (`wamsoft/msgreceiver`); the same host seam, and no script surface of its own |

### The names this inventory does not cover

Sixteen more names are in the reference's own plugin table, and neither the
organisation publishes them nor the engine builds them in: `PackinOne.dll`,
`util_generic.dll`, `util_graph.dll`, `util_system.dll`, `lzfs.dll`,
`multiimage.dll`, `kirikiroid2.dll`, `KaichoTrans.dll`, `ExtKAGParser.dll`,
`KAGParserExb.dll`, `gfxEffect.dll`, `layerExAlpha.dll`, `layerExShimmer.dll`,
`sqlite3.dll`, `DrawDeviceD3D.dll` and `DrawDeviceD3DZ.dll`.

Their source is not published, so what each one installs is not known until
its build is analysed; the oracle is then the shipped build — the file name
and the sha256[:12] of that build, with the bytes in a committed fixture.
The project's plugin section carries the work each one needs.

## Conventions used by the plugin modules

* **A plugin is thin.** The module marshals arguments into an engine
  operation and marshals the result back. Pixel work, layout, audio mixing and
  container parsing belong to the engine and platform crates; a plugin module
  keeps only the dispatch state that has no home there.
* **State is per engine, never process-global.** Per-object payloads live in
  the native instance; per-engine state lives in the compatibility services'
  dispatch state.
* **Errors are typed.** A member that cannot do what the reference does raises
  the reference's error rather than returning a substitute value. The null
  plugin object is the only exception, and it warns on every call.
* **The reference is the oracle.** Every behaviour a script can observe
  matches `reference/cpp/**`; a knowingly-left divergence is recorded in the
  plugin's own page under *Known gaps*.
* **The surface is locked.** `krkr-gate surface-lock` fails when a registered
  member is added or removed, and `krkr-gate plugin-parity` fails when a
  reference member goes missing. Both are part of the gate.

## Reading order for a plugin page

Each page has the same sections:

1. **Surface** — the DLL name(s), the class or global it installs, and the
   registered member list.
2. **Behaviour** — what the members do, in the terms a script sees.
3. **State and lifetime** — where the per-object and per-engine state lives
   and when it is released.
4. **Errors** — what raises, and with which message.
5. **Tests** — the test files that pin the behaviour.
6. **Known gaps** — the disclosed divergences from the reference.
