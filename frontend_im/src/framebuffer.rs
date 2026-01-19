use crossbeam_channel::Receiver;
use snow_core::renderer::DisplayBuffer;

unsafe extern "C" {
    fn js_did_open_video(width: u32, height: u32);
    fn js_blit(buf_ptr: *const u8, buf_size: u32);
}

pub struct Sender {
    receiver: Receiver<DisplayBuffer>,
    current_width: u16,
    current_height: u16,
}

impl Sender {
    pub fn new(receiver: Receiver<DisplayBuffer>) -> Self {
        Self {
            receiver,
            current_width: 0,
            current_height: 0,
        }
    }

    pub fn tick(&mut self) {
        while let Ok(frame) = self.receiver.try_recv() {
            self.send_frame(frame);
        }
    }

    fn send_frame(&mut self, frame: DisplayBuffer) {
        let width = frame.width();
        let height = frame.height();

        if width != self.current_width || height != self.current_height {
            unsafe {
                js_did_open_video(width as u32, height as u32);
            }
            self.current_width = width;
            self.current_height = height;
        }

        let data = frame.into_inner();
        if !data.is_empty() {
            unsafe {
                js_blit(data.as_ptr(), data.len() as u32);
            }
        }
    }
}
