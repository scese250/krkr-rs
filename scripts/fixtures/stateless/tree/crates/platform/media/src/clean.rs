//! Fixture for the `stateless` guard: the clean near-miss.  The service is
//! built with its state and only reads it afterwards.

pub struct Media {
    volume: f32,
}

impl Media {
    pub fn new() -> Self {
        Media { volume: 1.0 }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }
}
