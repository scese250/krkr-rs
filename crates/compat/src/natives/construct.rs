//! The one place a compatibility native builds a fresh script object.
//!
//! A native that hands a script a container — an array of fields, a
//! dictionary of named members — needs the same object the script's own
//! literal produces. The VM's constructors build exactly that object:
//! [`Tjs2Engine::new_array`] creates the array class and
//! [`Tjs2Engine::new_dictionary`] the dictionary class, without a script
//! being compiled or run.
//!
//! Evaluating a literal instead costs a compile and a run per container, and
//! the containers are built per line, per character or per file entry. The
//! helpers here are the single owner of that construction: a native asks for
//! an array or a dictionary and fills it through the VM's own member writes.
//!
//! The objects a script sees are the same either way: the same class, the
//! same members, the same order and the same error when a write fails.

use tjs::{DetachedValue, Tjs2Engine, TjsValue};

/// A fresh TJS array: the VM's own array class, the object an array literal
/// produces.
///
/// # Errors
///
/// Returns the VM's error when the array cannot be created.
pub(crate) fn array(host: &Tjs2Engine) -> Result<DetachedValue, String> {
    host.new_array()
}

/// A fresh TJS dictionary: the VM's own dictionary class, the object a
/// dictionary literal produces.
///
/// # Errors
///
/// Returns the VM's error when the dictionary cannot be created.
pub(crate) fn dictionary(host: &Tjs2Engine) -> Result<DetachedValue, String> {
    host.new_dictionary()
}

/// A fresh TJS array holding `values`, in the order they are given.
///
/// Every item is written with the array's own indexed write, so the array is
/// the one a script literal of the same values produces.
///
/// # Errors
///
/// Returns the VM's error when the array cannot be created, and the write's
/// error when an item cannot be stored.
pub(crate) fn array_of(host: &Tjs2Engine, values: &[TjsValue]) -> Result<DetachedValue, String> {
    let array = array(host)?;
    for (index, value) in values.iter().enumerate() {
        host.set_member(array.raw_id(), &index.to_string(), value)
            .map_err(|error| format!("array item {index}: {error}"))?;
    }
    Ok(array)
}

/// A fresh TJS dictionary holding `entries`, in the order they are given.
///
/// The members are written in that order, so a script that enumerates the
/// dictionary sees them in the order the caller listed them.
///
/// # Errors
///
/// Returns the VM's error when the dictionary cannot be created, and the
/// write's error when a member cannot be stored.
pub(crate) fn dictionary_of(
    host: &Tjs2Engine,
    entries: &[(&str, TjsValue)],
) -> Result<DetachedValue, String> {
    let dictionary = dictionary(host)?;
    for (key, value) in entries {
        host.set_member(dictionary.raw_id(), key, value)
            .map_err(|error| format!("dictionary member {key}: {error}"))?;
    }
    Ok(dictionary)
}
