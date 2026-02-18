use anyhow::Result;
use snow_core::mac::scsi::ethernet::EthernetBackend;

use crate::js_api;

/// Ethernet backend that tunnels raw Ethernet frames over a WebSocket.
///
/// The JS side (implemented in the Infinite Mac repo) manages the WebSocket
/// connection and maintains a queue of received frames. This struct calls into
/// JS via Emscripten's library mechanism (see js_api/exports.js for the stubs
/// and js_api/network.rs for the FFI declarations).
///
/// Frame format: one raw Ethernet frame per WebSocket binary message, in both
/// directions. No length prefix or framing is needed since WebSocket messages
/// already have a defined length boundary.
pub struct WsEthernetBackend {
    handle: i32,
}

impl WsEthernetBackend {
    /// Connect to the given WebSocket relay URL.
    ///
    /// Returns None if the JS side reported an error opening the connection.
    /// Note: the WebSocket handshake is asynchronous; packets sent before the
    /// handshake completes are queued by the JS side and sent once connected.
    pub fn new(url: &str) -> Option<Self> {
        let handle = js_api::network::ws_connect(url);
        if handle < 0 {
            log::error!("Failed to open network relay WebSocket to {}", url);
            None
        } else {
            log::info!("Network relay WebSocket opened: handle={}", handle);
            Some(Self { handle })
        }
    }
}

impl EthernetBackend for WsEthernetBackend {
    fn send(&mut self, packet: &[u8]) -> Result<()> {
        js_api::network::ws_send(self.handle, packet);
        Ok(())
    }

    fn has_pending(&self) -> bool {
        js_api::network::ws_has_pending(self.handle)
    }

    fn try_recv(&mut self) -> Option<Vec<u8>> {
        js_api::network::ws_recv(self.handle)
    }
}

impl Drop for WsEthernetBackend {
    fn drop(&mut self) {
        js_api::network::ws_close(self.handle);
    }
}
