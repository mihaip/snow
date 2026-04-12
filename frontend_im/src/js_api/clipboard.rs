use std::ffi::CString;
use std::os::raw::c_char;

extern "C" {
    fn js_set_clipboard_text(text: *const c_char);
}

pub fn set_text(text: &str) {
    let Ok(text) = CString::new(text) else {
        log::warn!("Skipping clipboard update containing an interior NUL byte");
        return;
    };

    unsafe {
        js_set_clipboard_text(text.as_ptr());
    }
}
