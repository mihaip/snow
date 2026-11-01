use std::sync::{Arc, Mutex};

use snow_core::renderer::DisplayBuffer;

use crate::js_api;

pub struct Sender {
    receiver: Arc<Mutex<Option<DisplayBuffer>>>,
    current_width: u16,
    current_height: u16,
}

impl Sender {
    pub fn new(receiver: Arc<Mutex<Option<DisplayBuffer>>>) -> Self {
        Self {
            receiver,
            current_width: 0,
            current_height: 0,
        }
    }

    pub fn tick(&mut self) {
        let frame = {
            let mut lock = self.receiver.lock().unwrap();
            // Wait for the core to have a frame _and_ for the browser to
            // request one. By not taking a queued frame if it's not needed, we
            // let the core skip generating extra frames (ChannelRenderer::frame_wanted()
            // is false), which improves performance.
            // Check the core first to avoid consuming and throwing away the
            // browser request.
            if lock.is_none() || !js_api::video::consume_frame_request() {
                return;
            }
            lock.take().unwrap()
        };
        self.send_frame(frame);
    }

    fn send_frame(&mut self, frame: DisplayBuffer) {
        let width = frame.width();
        let height = frame.height();

        if width != self.current_width || height != self.current_height {
            js_api::video::did_open(width as u32, height as u32);
            self.current_width = width;
            self.current_height = height;
        }

        let data = frame.into_inner();
        js_api::video::blit(&data);
    }
}
