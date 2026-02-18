extern "C" {
    /// Open a network channel. Returns a handle >= 0 on success, -1 on failure.
    /// The JS side decides what transport to use (e.g. WebSocket) and where to
    /// connect; the Rust side treats the handle as opaque.
    fn js_network_open() -> i32;

    /// Send binary data over a network channel.
    fn js_network_send(handle: i32, buf_ptr: *const u8, buf_len: usize);

    /// Check whether at least one received packet is waiting in the JS-side queue.
    /// Returns 1 if data is available, 0 if not.
    fn js_network_has_pending(handle: i32) -> i32;

    /// Pop the oldest received packet from the JS-side queue into the caller's buffer.
    /// Returns the number of bytes written (> 0), 0 if the queue is empty,
    /// or -1 if the channel has been closed or errored.
    fn js_network_recv(handle: i32, buf_ptr: *mut u8, buf_capacity: usize) -> i32;

    /// Close a network channel.
    fn js_network_close(handle: i32);
}

/// Maximum Ethernet frame size (1518 bytes + some headroom)
const MAX_FRAME_SIZE: usize = 1600;

/// Open a network channel. Returns the handle, or -1 on failure.
pub fn network_open() -> i32 {
    unsafe { js_network_open() }
}

pub fn network_send(handle: i32, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    unsafe { js_network_send(handle, data.as_ptr(), data.len()) }
}

pub fn network_has_pending(handle: i32) -> bool {
    unsafe { js_network_has_pending(handle) != 0 }
}

/// Returns the next received packet, or None if the queue is empty or the
/// channel is closed.
pub fn network_recv(handle: i32) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; MAX_FRAME_SIZE];
    let n = unsafe { js_network_recv(handle, buf.as_mut_ptr(), buf.len()) };
    if n <= 0 {
        None
    } else {
        buf.truncate(n as usize);
        Some(buf)
    }
}

pub fn network_close(handle: i32) {
    unsafe { js_network_close(handle) }
}
