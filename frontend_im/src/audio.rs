use anyhow::Result;
use snow_core::renderer::{
    null_audio_sink, AudioBuffer, AudioProvider, AudioSink, AUDIO_QUEUE_LEN,
};

use crate::js_api;

const SAMPLE_SIZE_BITS: u32 = 32;

const AUDIO_WORKLET_QUANTUM_FRAMES: u32 = 128;
const BYTES_PER_SAMPLE: usize = std::mem::size_of::<f32>();

pub struct JsAudioProvider {
    stream_opened: bool,
}

impl JsAudioProvider {
    pub fn new() -> Self {
        Self {
            stream_opened: false,
        }
    }
}

impl AudioProvider for JsAudioProvider {
    fn create_stream(
        &mut self,
        freq: i32,
        channels: u8,
        samples: u16,
    ) -> Result<Box<dyn AudioSink>> {
        if self.stream_opened {
            log::info!(
                "Ignoring additional JS audio stream: sample_rate={} channels={} samples={}",
                freq,
                channels,
                samples
            );
            return Ok(null_audio_sink());
        }
        self.stream_opened = true;
        Ok(Box::new(JsAudioSink::new(freq, channels, samples)))
    }
}

struct JsAudioSink {
    bytes_per_second: usize,
    max_js_buffer_bytes: usize,
    audio_worklet_quantum_seconds: f64,
}

impl JsAudioSink {
    fn new(freq: i32, channels: u8, samples: u16) -> Self {
        let sample_rate = u32::try_from(freq).unwrap();
        let channels_usize = usize::from(channels);
        let bytes_per_second = sample_rate as usize * channels_usize * BYTES_PER_SAMPLE;
        let max_js_buffer_bytes =
            usize::from(samples) * channels_usize * BYTES_PER_SAMPLE * AUDIO_QUEUE_LEN;
        let audio_worklet_quantum_seconds =
            AUDIO_WORKLET_QUANTUM_FRAMES as f64 / f64::from(sample_rate);

        js_api::audio::did_open(sample_rate, SAMPLE_SIZE_BITS, u32::from(channels));
        Self {
            bytes_per_second,
            max_js_buffer_bytes,
            audio_worklet_quantum_seconds,
        }
    }
}

impl AudioSink for JsAudioSink {
    fn send(&self, buffer: AudioBuffer) -> Result<()> {
        let expected_len = buffer.len() * BYTES_PER_SAMPLE;
        let max_fill = self.max_js_buffer_bytes.saturating_sub(expected_len);
        loop {
            let js_buffer_size = js_api::audio::buffer_size();
            if js_buffer_size < 0 || js_buffer_size as usize <= max_fill {
                break;
            }
            // Estimate how long it will take to drain the buffer so that we
            // can sleep instead of spinning. Leave some headroom for jitter,
            // and wait at most one audio worklet quantum.
            let wait_bytes = js_buffer_size as usize - max_fill;
            let wait_seconds = ((wait_bytes as f64 / self.bytes_per_second as f64) * 0.75)
                .clamp(0.0, self.audio_worklet_quantum_seconds);
            js_api::runtime::sleep_seconds(wait_seconds);
        }

        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const u8, expected_len) };
        js_api::audio::enqueue(bytes);
        Ok(())
    }

    fn is_empty(&self) -> bool {
        js_api::audio::buffer_size() <= 0
    }

    fn is_full(&self) -> bool {
        let js_buffer_size = js_api::audio::buffer_size();
        js_buffer_size >= 0 && (js_buffer_size as usize) >= self.max_js_buffer_bytes
    }
}
