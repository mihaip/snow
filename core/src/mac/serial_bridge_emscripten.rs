//! Serial port bridge stub for Emscripten builds
//!
//! This module provides stub implementations for serial bridge types
//! to allow the codebase to compile for Emscripten without the actual
//! serial bridge functionality (which requires native platform features).

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SerialBridgeConfig {
    Pty,
    Tcp(u16),
    LocalTalk,
}

impl std::fmt::Display for SerialBridgeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SerialBridgeConfig")
    }
}

/// Status of an active serial bridge (stub for Emscripten)
#[derive(Debug, Clone)]
pub enum SerialBridgeStatus {
    Pty(PathBuf),
    TcpListening(u16),
    TcpConnected(u16, String),
    LocalTalk(String),
}

impl std::fmt::Display for SerialBridgeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SerialBridgeStatus")
    }
}

pub struct SccBridge;

impl SccBridge {
    pub fn new(_config: &SerialBridgeConfig) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Serial bridges are not supported on Emscripten",
        ))
    }

    pub fn write_from_scc(&mut self, _data: &[u8]) {}

    pub fn read_to_scc(&mut self) -> Vec<u8> {
        Vec::new()
    }

    pub fn poll(&mut self) -> bool {
        false
    }

    pub fn status(&self) -> SerialBridgeStatus {
        SerialBridgeStatus::TcpListening(0)
    }

    pub fn is_localtalk(&self) -> bool {
        false
    }
}
