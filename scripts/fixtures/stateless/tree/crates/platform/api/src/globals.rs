//! Fixture for the `stateless` guard: the process-global state the guard
//! must catch.  Every forbidden pattern appears once, so the self-test proves
//! that each rule still fires.

pub struct Counters;

pub static mut WRITE_COUNT: u64 = 0;

thread_local! {
    pub static LAST_NAME: String = String::new();
}

lazy_static! {
    pub static ref TABLE: Vec<u8> = Vec::new();
}

pub static CACHE: LazyLock<Vec<u8>> = LazyLock::new(Vec::new);

pub static READY: OnceLock<bool> = OnceLock::new();

pub static SLOT: OnceCell<u32> = OnceCell::new();
