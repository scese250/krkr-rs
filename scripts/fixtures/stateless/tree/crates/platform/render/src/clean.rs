//! Fixture for the `stateless` guard: the clean near-miss.  Every method
//! takes `&self`, so the service holds no mutable state of its own.

pub struct RenderPipeline {
    scale: f32,
}

impl RenderPipeline {
    pub fn new() -> Self {
        RenderPipeline { scale: 1.0 }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn with_scale(&self, scale: f32) -> Self {
        RenderPipeline { scale }
    }
}
