//! Fixture for the `stateless` guard: a media service that keeps its state
//! in itself.  The method that mutates the service must be caught.

pub struct Media {
    volume: f32,
}

impl Media {
    pub fn new() -> Self {
        Media { volume: 1.0 }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
    }
}
