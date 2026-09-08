//! Read-only inspector bridge. The borrowed mirror is valid only during the
//! synchronous JS call; the worker copies resource bytes it needs to retain.
extern "C" {
    fn js_inspector_call(
        ptr: *const u8,
        len: usize,
        source: u32,
        returning: u32,
        handle: u32,
        reference: u32,
        resource_type: u32,
        pc: u32,
    );
    fn js_inspector_before_close(ptr: *const u8, len: usize);
    fn js_inspector_initialized(model: u32);
    fn js_inspector_active() -> i32;
    fn js_inspector_tick(ptr: *const u8, len: usize);
}

pub fn initialized(model: u32) {
    unsafe { js_inspector_initialized(model) }
}

pub fn tick(memory: &[u8]) {
    unsafe {
        if js_inspector_active() != 0 {
            js_inspector_tick(memory.as_ptr(), memory.len());
        }
    }
}

pub fn active() -> bool {
    unsafe { js_inspector_active() != 0 }
}

pub fn before_resource_file_close(memory: &[u8]) {
    unsafe { js_inspector_before_close(memory.as_ptr(), memory.len()) }
}

pub fn call_observation(
    memory: &[u8],
    source: u32,
    returning: bool,
    handle: u32,
    reference: u32,
    resource_type: u32,
    pc: u32,
) {
    unsafe {
        js_inspector_call(
            memory.as_ptr(),
            memory.len(),
            source,
            returning as u32,
            handle,
            reference,
            resource_type,
            pc,
        )
    }
}
