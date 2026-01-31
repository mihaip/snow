extern "C" {
    fn js_did_open_audio(sample_rate: u32, sample_size: u32, channels: u32);
    fn js_audio_buffer_size() -> i32;
    fn js_enqueue_audio(buf_ptr: *const u8, buf_size: u32);
}

pub fn did_open(sample_rate: u32, sample_size: u32, channels: u32) {
    unsafe {
        js_did_open_audio(sample_rate, sample_size, channels);
    }
}

pub fn buffer_size() -> i32 {
    unsafe { js_audio_buffer_size() }
}

pub fn enqueue(buffer: &[u8]) {
    if buffer.is_empty() {
        return;
    }
    let len_u32 = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    unsafe {
        js_enqueue_audio(buffer.as_ptr(), len_u32);
    }
}
