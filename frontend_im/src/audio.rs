use anyhow::Result;
use snow_core::renderer::{AudioBuffer, AudioSink, AUDIO_BUFFER_SIZE, AUDIO_QUEUE_LEN};

use crate::js_api;

// Matches the configuration in SDLAudioSink (and thus the monitor horizontal
// sync rate)
const SAMPLE_RATE: u32 = 22_050;
const SAMPLE_SIZE_BITS: u32 = 32;
const CHANNELS: u32 = 2;

const AUDIO_WORKLET_QUANTUM_FRAMES: u32 = 128;
const AUDIO_WORKLET_QUANTUM_SECONDS: f64 = AUDIO_WORKLET_QUANTUM_FRAMES as f64 / SAMPLE_RATE as f64;

const BYTES_PER_SAMPLE: usize = (SAMPLE_SIZE_BITS / 8) as usize;
const BYTES_PER_SECOND: usize = SAMPLE_RATE as usize * CHANNELS as usize * BYTES_PER_SAMPLE;

// Mirror the blocking behavior of ChannelAudioSink which has a bounded channel
const MAX_JS_BUFFER_BYTES: usize = AUDIO_BUFFER_SIZE * BYTES_PER_SAMPLE * AUDIO_QUEUE_LEN;

pub struct JsAudioSink;

impl JsAudioSink {
    pub fn new() -> Self {
        js_api::audio::did_open(SAMPLE_RATE, SAMPLE_SIZE_BITS, CHANNELS);
        Self
    }
}

impl AudioSink for JsAudioSink {
    fn send(&mut self, buffer: AudioBuffer) -> Result<()> {
        let expected_len = buffer.len() * BYTES_PER_SAMPLE;
        let max_fill = MAX_JS_BUFFER_BYTES.saturating_sub(expected_len);
        loop {
            let js_buffer_size = js_api::audio::buffer_size();
            if js_buffer_size < 0 || js_buffer_size as usize <= max_fill {
                break;
            }
            // Estimate how long it will take to drain the buffer so that we
            // can sleep instead of spinning. Leave some headroom for jitter,
            // and wait at most one audio worklet quantum.
            let wait_bytes = js_buffer_size as usize - max_fill;
            let wait_seconds = ((wait_bytes as f64 / BYTES_PER_SECOND as f64) * 0.75)
                .clamp(0.0, AUDIO_WORKLET_QUANTUM_SECONDS);
            js_api::runtime::sleep_seconds(wait_seconds);
        }

        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const u8, expected_len) };
        js_api::audio::enqueue(bytes);
        Ok(())
    }
}
