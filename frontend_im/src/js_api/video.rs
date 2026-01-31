extern "C" {
    fn js_did_open_video(width: u32, height: u32);
    fn js_blit(buf_ptr: *const u8, buf_size: u32);
}

pub fn did_open(width: u32, height: u32) {
    unsafe {
        js_did_open_video(width, height);
    }
}

pub fn blit(frame: &[u8]) {
    if frame.is_empty() {
        return;
    }
    let len_u32 = u32::try_from(frame.len()).unwrap_or(u32::MAX);
    unsafe {
        js_blit(frame.as_ptr(), len_u32);
    }
}
