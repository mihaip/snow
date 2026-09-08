//! Read-only inspector bridge. The borrowed mirror is valid only during the
//! synchronous JS call; the worker copies resource bytes it needs to retain.
extern "C" {
    fn js_inspector_before_close(ptr: *const u8, len: usize);
    fn js_inspector_initialized(model: u32);
    fn js_inspector_active() -> i32;
    fn js_inspector_capture(ptr: *const u8, len: usize);
}

pub fn initialized(model: u32) {
    unsafe { js_inspector_initialized(model) }
}

pub fn capture(memory: &[u8]) {
    unsafe {
        if js_inspector_active() != 0 {
            js_inspector_capture(memory.as_ptr(), memory.len());
        }
    }
}

pub fn active() -> bool {
    unsafe { js_inspector_active() != 0 }
}

pub fn before_resource_file_close(memory: &[u8]) {
    unsafe { js_inspector_before_close(memory.as_ptr(), memory.len()) }
}
