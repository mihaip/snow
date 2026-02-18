use anyhow::Result;
use snow_core::mac::scsi::ethernet::EthernetBackend;

use crate::js_api;

/// Ethernet backend that delegates to the JS side via an opaque network handle.
///
/// The JS side (implemented in the Infinite Mac repo) decides what transport to
/// use (e.g. a WebSocket relay) and manages the connection lifecycle. This
/// struct calls into JS via Emscripten's library mechanism (see
/// js_api/exports.js for the stubs and js_api/network.rs for the FFI
/// declarations).
///
/// Frame format: one raw Ethernet frame per message, in both directions.
pub struct JsEthernetBackend {
    handle: i32,
}

impl JsEthernetBackend {
    /// Ask the JS side to open a network channel.
    ///
    /// Returns None if the JS side reported an error.
    pub fn new() -> Option<Self> {
        let handle = js_api::network::network_open();
        if handle < 0 {
            log::error!("Failed to open network channel");
            None
        } else {
            log::info!("Network channel opened: handle={}", handle);
            Some(Self { handle })
        }
    }
}

impl EthernetBackend for JsEthernetBackend {
    fn send(&mut self, packet: &[u8]) -> Result<()> {
        js_api::network::network_send(self.handle, packet);
        Ok(())
    }

    fn has_pending(&self) -> bool {
        js_api::network::network_has_pending(self.handle)
    }

    fn try_recv(&mut self) -> Option<Vec<u8>> {
        js_api::network::network_recv(self.handle)
    }
}

impl Drop for JsEthernetBackend {
    fn drop(&mut self) {
        js_api::network::network_close(self.handle);
    }
}
