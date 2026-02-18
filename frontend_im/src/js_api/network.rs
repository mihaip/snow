extern "C" {
    /// Open a WebSocket connection to the given URL.
    /// Returns a handle >= 0 on success, -1 if the connection could not be initiated.
    /// The connection may not be fully established yet when this returns; packets
    /// sent before the handshake completes are queued by the JS side.
    fn js_ws_connect(url_ptr: *const u8, url_len: usize) -> i32;

    /// Send binary data over a WebSocket connection.
    fn js_ws_send(handle: i32, buf_ptr: *const u8, buf_len: usize);

    /// Check whether at least one received packet is waiting in the JS-side queue.
    /// Returns 1 if data is available, 0 if not.
    fn js_ws_has_pending(handle: i32) -> i32;

    /// Pop the oldest received packet from the JS-side queue into the caller's buffer.
    /// Returns the number of bytes written (> 0), 0 if the queue is empty,
    /// or -1 if the connection has been closed or errored.
    fn js_ws_recv(handle: i32, buf_ptr: *mut u8, buf_capacity: usize) -> i32;

    /// Close a WebSocket connection.
    fn js_ws_close(handle: i32);
}

/// Maximum Ethernet frame size (1518 bytes + some headroom)
const MAX_FRAME_SIZE: usize = 1600;

/// Open a WebSocket to the given URL. Returns the handle, or -1 on failure.
pub fn ws_connect(url: &str) -> i32 {
    unsafe { js_ws_connect(url.as_ptr(), url.len()) }
}

pub fn ws_send(handle: i32, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    unsafe { js_ws_send(handle, data.as_ptr(), data.len()) }
}

pub fn ws_has_pending(handle: i32) -> bool {
    unsafe { js_ws_has_pending(handle) != 0 }
}

/// Returns the next received packet, or None if the queue is empty or the
/// connection is closed.
pub fn ws_recv(handle: i32) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; MAX_FRAME_SIZE];
    let n = unsafe { js_ws_recv(handle, buf.as_mut_ptr(), buf.len()) };
    if n <= 0 {
        None
    } else {
        buf.truncate(n as usize);
        Some(buf)
    }
}

pub fn ws_close(handle: i32) {
    unsafe { js_ws_close(handle) }
}
