//! The header is patched on every flush, so the file is valid up to the last flush however the
//! process ends.

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy::log::{info, warn};
use kira::Frame;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;

const RING_SECONDS: usize = 8;
const FLUSH_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// Installs a tap writing `path` and returns its frame clock, the count of frames tapped. `None`
/// when the file cannot be created or its writer cannot start.
pub(crate) fn install_at(
    builder: &mut kira::track::MainTrackBuilder,
    path: &Path,
    sample_rate: u32,
) -> Option<Arc<AtomicU64>> {
    let file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            warn!("mix tap: cannot create {}: {e}", path.display());
            return None;
        }
    };
    let (producer, consumer) = rtrb::RingBuffer::new(sample_rate as usize * 2 * RING_SECONDS);
    let spawned = std::thread::Builder::new()
        .name("mix-tap".into())
        .spawn(move || writer(file, consumer, sample_rate));
    if let Err(e) = spawned {
        warn!("mix tap: no writer thread: {e}");
        return None;
    }
    info!("mix tap: recording to {}", path.display());
    let frames = Arc::new(AtomicU64::new(0));
    builder.add_effect(TapBuilder {
        producer,
        frames: Arc::clone(&frames),
    });
    Some(frames)
}

struct TapBuilder {
    producer: rtrb::Producer<f32>,
    frames: Arc<AtomicU64>,
}

impl EffectBuilder for TapBuilder {
    type Handle = ();
    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        (
            Box::new(Tap {
                producer: self.producer,
                frames: self.frames,
            }),
            (),
        )
    }
}

struct Tap {
    producer: rtrb::Producer<f32>,
    frames: Arc<AtomicU64>,
}

impl Effect for Tap {
    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info<'_>) {
        for f in input.iter() {
            let _ = self.producer.push(f.left);
            let _ = self.producer.push(f.right);
        }
        self.frames.fetch_add(input.len() as u64, Ordering::Relaxed);
    }
}

fn writer(mut file: std::fs::File, mut consumer: rtrb::Consumer<f32>, sample_rate: u32) {
    if let Err(e) = file.write_all(&wav_header(sample_rate, 0)) {
        warn!("mix tap: header write failed: {e}");
        return;
    }
    let mut samples_written: u64 = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    loop {
        let done = consumer.is_abandoned();
        buf.clear();
        while let Ok(s) = consumer.pop() {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        if !buf.is_empty() {
            if let Err(e) = file.write_all(&buf) {
                warn!("mix tap: write failed: {e}");
                return;
            }
            samples_written += (buf.len() / 4) as u64;
            if file.seek(SeekFrom::Start(0)).is_ok() {
                let _ = file.write_all(&wav_header(sample_rate, samples_written));
                let _ = file.seek(SeekFrom::End(0));
            }
        }
        if done {
            info!(
                "mix tap: closed, {:.1} s recorded",
                samples_written as f64 / 2.0 / f64::from(sample_rate)
            );
            return;
        }
        std::thread::sleep(FLUSH_EVERY);
    }
}

/// A stereo IEEE float WAV header for `samples` samples across both channels.
pub(crate) fn wav_header(sample_rate: u32, samples: u64) -> [u8; 44] {
    let data_bytes = (samples * 4) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&3u16.to_le_bytes());
    h[22..24].copy_from_slice(&2u16.to_le_bytes());
    h[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    h[28..32].copy_from_slice(&(sample_rate * 8).to_le_bytes());
    h[32..34].copy_from_slice(&8u16.to_le_bytes());
    h[34..36].copy_from_slice(&32u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_decodes_through_kira() {
        let mut file = Vec::new();
        file.extend_from_slice(&wav_header(48_000, 4));
        for s in [0.5f32, -0.5, 0.25, -0.25] {
            file.extend_from_slice(&s.to_le_bytes());
        }
        let sound =
            kira::sound::static_sound::StaticSoundData::from_cursor(std::io::Cursor::new(file))
                .expect("tap WAV decodes");
        assert_eq!(sound.frames.len(), 2);
        assert!((sound.frames[1].right + 0.25).abs() < 1e-6);
    }
}
