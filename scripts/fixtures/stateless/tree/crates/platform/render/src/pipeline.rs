//! Fixture for the `stateless` guard: a service that keeps its state in
//! itself.  The method that mutates the service must be caught.

pub struct RenderPipeline {
    scale: f32,
}

impl RenderPipeline {
    pub fn new() -> Self {
        RenderPipeline { scale: 1.0 }
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }
}
