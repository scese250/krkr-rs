//! `extNagano.dll` plugin port: Nagano extended transition types.
//!
//! The reference plugin's `V2Link` registers transition **handler providers**
//! into the engine's global registry; `Layer.beginTransition` then looks them
//! up by name. In krkr-rs the handlers are built into L2 (`engine_api`)
//! and dispatched by `Layer.beginTransition`, so this module registers no TJS
//! classes.
//!
//! Implemented provider names: `blurfade` and `rgbfade`.

use crate::{CompatCtx, CompatError};

/// No TJS surface: the transition handlers are built into L2.
pub fn register(_ctx: &mut CompatCtx) -> Result<(), CompatError> {
    Ok(())
}
