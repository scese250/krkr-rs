//! The native `Layer.type` (`tTVPLayerType`) vocabulary and what every
//! type means for the layer it is written on.
//!
//! L2 stores the generic blend vocabulary directly ([`BlendMode`]); the
//! integer numbering scripts read and write as `layer.type` is TVP vocabulary
//! and therefore lives in L3. Every documented native type maps to its real
//! behaviour, so the extended Photoshop blends survive the compat seam instead
//! of collapsing to source-over.
//!
//! One table gives every type its three meanings — the blend it draws with,
//! the neutral colour it gives the layer and whether it carries an image at
//! all — so a type that is written on a layer reaches every part of the layer
//! it names.
//!
//! Reference: `tTVPLayerType` (`reference/cpp/core/visual/drawable.h:20`).

use api::blend::{BlendMode, DstBlend};

use crate::engine_api::DestAlpha;

/// `ltBinder` — a container that draws nothing.
pub(super) const LT_BINDER: i64 = 0;
/// `ltOpaque`/`ltCoverRect`.
pub(super) const LT_OPAQUE: i64 = 1;
/// `ltAlpha`/`ltTransparent` (the scene default).
pub(super) const LT_ALPHA: i64 = 2;
/// `ltAdditive`.
pub(super) const LT_ADDITIVE: i64 = 3;
/// `ltSubtractive`.
pub(super) const LT_SUBTRACTIVE: i64 = 4;
/// `ltMultiplicative`.
pub(super) const LT_MULTIPLICATIVE: i64 = 5;
/// `ltEffect` — layer-effect callback, not a bitmap blend.
pub(super) const LT_EFFECT: i64 = 6;
/// `ltFilter` — layer-filter callback, not a bitmap blend.
pub(super) const LT_FILTER: i64 = 7;
/// `ltDodge` — colour dodge.
pub(super) const LT_DODGE: i64 = 8;
/// `ltDarken` — per-channel minimum.
pub(super) const LT_DARKEN: i64 = 9;
/// `ltLighten` — per-channel maximum.
pub(super) const LT_LIGHTEN: i64 = 10;
/// `ltScreen`.
pub(super) const LT_SCREEN: i64 = 11;
/// `ltAddAlpha` — additive with an explicit alpha channel.
pub(super) const LT_ADD_ALPHA: i64 = 12;
/// `ltPsNormal` — Photoshop-style normal (plain source-over).
pub(super) const LT_PS_NORMAL: i64 = 13;
/// `ltPsAdditive`.
pub(super) const LT_PS_ADDITIVE: i64 = 14;
/// `ltPsSubtractive`.
pub(super) const LT_PS_SUBTRACTIVE: i64 = 15;
/// `ltPsMultiplicative`.
pub(super) const LT_PS_MULTIPLICATIVE: i64 = 16;
/// `ltPsScreen`.
pub(super) const LT_PS_SCREEN: i64 = 17;
/// `ltPsOverlay`.
pub(super) const LT_PS_OVERLAY: i64 = 18;
/// `ltPsHardLight`.
pub(super) const LT_PS_HARD_LIGHT: i64 = 19;
/// `ltPsSoftLight`.
pub(super) const LT_PS_SOFT_LIGHT: i64 = 20;
/// `ltPsColorDodge`.
pub(super) const LT_PS_COLOR_DODGE: i64 = 21;
/// `ltPsColorDodge5` — the legacy KiriKiri approximation.
pub(super) const LT_PS_COLOR_DODGE5: i64 = 22;
/// `ltPsColorBurn`.
pub(super) const LT_PS_COLOR_BURN: i64 = 23;
/// `ltPsLighten`.
pub(super) const LT_PS_LIGHTEN: i64 = 24;
/// `ltPsDarken`.
pub(super) const LT_PS_DARKEN: i64 = 25;
/// `ltPsDifference`.
pub(super) const LT_PS_DIFFERENCE: i64 = 26;
/// `ltPsDifference5` — the legacy KiriKiri approximation.
pub(super) const LT_PS_DIFFERENCE5: i64 = 27;
/// `ltPsExclusion`.
pub(super) const LT_PS_EXCLUSION: i64 = 28;

/// The neutral colour of a type whose image is the transparent white.
const TRANSPARENT_WHITE: i64 = 0x00ff_ffff;
/// The neutral colour of a type whose image is the transparent black.
const TRANSPARENT_BLACK: i64 = 0x0000_0000;
/// The neutral colour of a type whose image is the transparent grey.
const TRANSPARENT_GREY: i64 = 0x0080_8080;

/// What a script `Layer.type` means for the layer it is written on.
#[derive(Clone, Copy)]
pub(super) struct TypeFacts {
    /// The blend the type draws the layer with.
    pub blend: BlendMode,
    /// The neutral colour the type gives the layer.
    pub neutral_color: i64,
    /// Whether the type carries an image at all.
    ///
    /// The container types — the binder, the effect and the filter types —
    /// hold no image, so a layer of one of those types has no pixels.
    pub carries_image: bool,
}

/// Build the facts of a type.
const fn facts(blend: BlendMode, neutral_color: i64, carries_image: bool) -> TypeFacts {
    TypeFacts {
        blend,
        neutral_color,
        carries_image,
    }
}

/// What a script `Layer.type` means for the layer it is written on.
///
/// Every documented type maps to its own behaviour, and the core and
/// Photoshop spellings map to distinct variants so the encoder can reproduce
/// the exact script value. The three types that name no bitmap blend — the
/// binder, the effect and the filter types — display as an opaque layer,
/// which is the face and the operation mode their display type resolves to,
/// and they carry no image. A value that names no type at all yields `None`:
/// the layer keeps the blend it has, and the type reads back as the value
/// that was written.
pub(super) const fn type_facts(value: i64) -> Option<TypeFacts> {
    Some(match value {
        LT_BINDER => facts(BlendMode::Replace, TRANSPARENT_WHITE, false),
        LT_OPAQUE => facts(BlendMode::Replace, TRANSPARENT_WHITE, true),
        LT_ALPHA => facts(BlendMode::SourceOver, TRANSPARENT_WHITE, true),
        LT_ADDITIVE => facts(BlendMode::Additive, TRANSPARENT_BLACK, true),
        LT_SUBTRACTIVE => facts(BlendMode::Subtractive, TRANSPARENT_WHITE, true),
        LT_MULTIPLICATIVE => facts(BlendMode::Multiply, TRANSPARENT_WHITE, true),
        LT_EFFECT => facts(BlendMode::Replace, TRANSPARENT_WHITE, false),
        LT_FILTER => facts(BlendMode::Replace, TRANSPARENT_WHITE, false),
        LT_DODGE => facts(
            BlendMode::DestinationDependent(DstBlend::ColorDodge),
            TRANSPARENT_BLACK,
            true,
        ),
        LT_DARKEN => facts(
            BlendMode::DestinationDependent(DstBlend::Darken),
            TRANSPARENT_WHITE,
            true,
        ),
        LT_LIGHTEN => facts(
            BlendMode::DestinationDependent(DstBlend::Lighten),
            TRANSPARENT_BLACK,
            true,
        ),
        LT_SCREEN => facts(BlendMode::Screen, TRANSPARENT_BLACK, true),
        LT_ADD_ALPHA => facts(BlendMode::AddAlpha, TRANSPARENT_BLACK, true),
        LT_PS_NORMAL => facts(BlendMode::PsNormal, TRANSPARENT_BLACK, true),
        LT_PS_ADDITIVE => facts(BlendMode::PsAdditive, TRANSPARENT_BLACK, true),
        LT_PS_SUBTRACTIVE => facts(BlendMode::PsSubtractive, TRANSPARENT_WHITE, true),
        LT_PS_MULTIPLICATIVE => facts(BlendMode::PsMultiplicative, TRANSPARENT_WHITE, true),
        LT_PS_SCREEN => facts(BlendMode::PsScreen, TRANSPARENT_BLACK, true),
        LT_PS_OVERLAY => facts(
            BlendMode::DestinationDependent(DstBlend::Overlay),
            TRANSPARENT_GREY,
            true,
        ),
        LT_PS_HARD_LIGHT => facts(
            BlendMode::DestinationDependent(DstBlend::HardLight),
            TRANSPARENT_GREY,
            true,
        ),
        LT_PS_SOFT_LIGHT => facts(
            BlendMode::DestinationDependent(DstBlend::SoftLight),
            TRANSPARENT_GREY,
            true,
        ),
        LT_PS_COLOR_DODGE => facts(
            BlendMode::DestinationDependent(DstBlend::PsColorDodge),
            TRANSPARENT_BLACK,
            true,
        ),
        LT_PS_COLOR_DODGE5 => facts(
            BlendMode::DestinationDependent(DstBlend::ColorDodge5),
            TRANSPARENT_BLACK,
            true,
        ),
        LT_PS_COLOR_BURN => facts(
            BlendMode::DestinationDependent(DstBlend::ColorBurn),
            TRANSPARENT_WHITE,
            true,
        ),
        LT_PS_LIGHTEN => facts(
            BlendMode::DestinationDependent(DstBlend::PsLighten),
            TRANSPARENT_BLACK,
            true,
        ),
        LT_PS_DARKEN => facts(
            BlendMode::DestinationDependent(DstBlend::PsDarken),
            TRANSPARENT_BLACK,
            true,
        ),
        LT_PS_DIFFERENCE => facts(
            BlendMode::DestinationDependent(DstBlend::Difference),
            TRANSPARENT_BLACK,
            true,
        ),
        LT_PS_DIFFERENCE5 => facts(
            BlendMode::DestinationDependent(DstBlend::Difference5),
            TRANSPARENT_BLACK,
            true,
        ),
        LT_PS_EXCLUSION => facts(
            BlendMode::DestinationDependent(DstBlend::Exclusion),
            TRANSPARENT_BLACK,
            true,
        ),
        _ => return None,
    })
}

/// Decode a script `Layer.type` into the render pipeline's generic blend mode.
///
/// The blend is one of the three meanings of a type; see [`type_facts`] for
/// the whole of them.
pub(crate) const fn from_native(value: i64) -> Option<BlendMode> {
    match type_facts(value) {
        None => None,
        Some(facts) => Some(facts.blend),
    }
}

/// The destination alpha handling a transition's mix is chosen by.
///
/// The mix follows the destination layer's type: a type that uses its alpha
/// channel gets the alpha-vs-alpha mix, the additive-alpha type gets the
/// additive mix, and every other type gets the plain mix.
///
/// The three types that name no bitmap blend — the binder, the effect and the
/// filter types — are read as the opaque one, because their display type
/// resolves to the opaque face.
pub(crate) const fn dest_alpha(blend: BlendMode) -> DestAlpha {
    match to_native(blend) {
        LT_ALPHA | LT_PS_NORMAL | LT_PS_ADDITIVE | LT_PS_SUBTRACTIVE | LT_PS_MULTIPLICATIVE
        | LT_PS_SCREEN | LT_PS_OVERLAY | LT_PS_HARD_LIGHT | LT_PS_SOFT_LIGHT
        | LT_PS_COLOR_DODGE | LT_PS_COLOR_DODGE5 | LT_PS_COLOR_BURN | LT_PS_LIGHTEN
        | LT_PS_DARKEN | LT_PS_DIFFERENCE | LT_PS_DIFFERENCE5 | LT_PS_EXCLUSION => {
            DestAlpha::Straight
        }
        LT_ADD_ALPHA => DestAlpha::Additive,
        _ => DestAlpha::Opaque,
    }
}

/// Encode a generic blend mode as its script `Layer.type`.
///
/// The inverse of [`from_native`]: every distinct native spelling round-trips
/// to itself, including the Photoshop spellings that share a blend formula
/// with a non-Photoshop type.
pub(super) const fn to_native(blend: BlendMode) -> i64 {
    match blend {
        BlendMode::Replace => LT_OPAQUE,
        BlendMode::SourceOver => LT_ALPHA,
        BlendMode::PsNormal => LT_PS_NORMAL,
        BlendMode::Additive => LT_ADDITIVE,
        BlendMode::PsAdditive => LT_PS_ADDITIVE,
        BlendMode::Subtractive => LT_SUBTRACTIVE,
        BlendMode::PsSubtractive => LT_PS_SUBTRACTIVE,
        BlendMode::Multiply => LT_MULTIPLICATIVE,
        BlendMode::PsMultiplicative => LT_PS_MULTIPLICATIVE,
        BlendMode::Screen => LT_SCREEN,
        BlendMode::PsScreen => LT_PS_SCREEN,
        BlendMode::AddAlpha => LT_ADD_ALPHA,
        BlendMode::DestinationDependent(DstBlend::ColorDodge) => LT_DODGE,
        BlendMode::DestinationDependent(DstBlend::PsColorDodge) => LT_PS_COLOR_DODGE,
        BlendMode::DestinationDependent(DstBlend::ColorDodge5) => LT_PS_COLOR_DODGE5,
        BlendMode::DestinationDependent(DstBlend::ColorBurn) => LT_PS_COLOR_BURN,
        BlendMode::DestinationDependent(DstBlend::Darken) => LT_DARKEN,
        BlendMode::DestinationDependent(DstBlend::PsDarken) => LT_PS_DARKEN,
        BlendMode::DestinationDependent(DstBlend::Lighten) => LT_LIGHTEN,
        BlendMode::DestinationDependent(DstBlend::PsLighten) => LT_PS_LIGHTEN,
        BlendMode::DestinationDependent(DstBlend::Difference) => LT_PS_DIFFERENCE,
        BlendMode::DestinationDependent(DstBlend::Difference5) => LT_PS_DIFFERENCE5,
        BlendMode::DestinationDependent(DstBlend::Exclusion) => LT_PS_EXCLUSION,
        BlendMode::DestinationDependent(DstBlend::Overlay) => LT_PS_OVERLAY,
        BlendMode::DestinationDependent(DstBlend::HardLight) => LT_PS_HARD_LIGHT,
        BlendMode::DestinationDependent(DstBlend::SoftLight) => LT_PS_SOFT_LIGHT,
    }
}
