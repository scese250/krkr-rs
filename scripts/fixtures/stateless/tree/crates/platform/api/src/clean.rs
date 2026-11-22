//! Fixture for the `stateless` guard: the clean near-miss.  An immutable
//! process-wide constant is not a mutable global, and a value passed in by
//! the caller is not state the service owns.

pub const MAX_NAME: usize = 64;

pub struct Name;

pub fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(MAX_NAME);
    out.push_str(name);
    out
}
