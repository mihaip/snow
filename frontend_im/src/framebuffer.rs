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
        while let Some(frame) = {
            let mut lock = self.receiver.lock().unwrap();
            lock.take()
        } {
            self.send_frame(frame);
        }
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
