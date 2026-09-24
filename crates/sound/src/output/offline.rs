//! No device: the renderer waits for the app to ask for the next frames.

use kira::backend::Renderer;

pub(crate) struct OfflineOutput {
    sample_rate: u32,
    renderer: Option<Renderer>,
    buffer: Vec<f32>,
}

impl OfflineOutput {
    pub(crate) fn new(sample_rate: u32, internal_buffer_size: usize) -> Self {
        Self {
            sample_rate,
            renderer: None,
            buffer: Vec::with_capacity(internal_buffer_size * 2),
        }
    }

    pub(crate) fn start(&mut self, renderer: Renderer) {
        self.renderer = Some(renderer);
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The next `frames` stereo frames, interleaved, with every command sent before the call
    /// applied from the first of them.
    pub(crate) fn render(&mut self, frames: usize) -> &[f32] {
        self.buffer.clear();
        self.buffer.resize(frames * 2, 0.0);
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.on_start_processing();
            renderer.process(&mut self.buffer, 2);
        }
        &self.buffer
    }
}
