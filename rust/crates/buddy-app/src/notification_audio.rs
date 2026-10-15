//! Bounded audio-file validation and cancellable previews. Device handles stay
//! on their owning worker thread. Tests decode fixtures without opening audio.

use std::io::{Cursor, Read as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use buddy_core::application_settings::{MAX_SOUND_BYTES, supported_sound_path};
use futures::channel::oneshot;
use rodio::{Decoder, DeviceSinkBuilder, Player};

fn read_audio(path: &str) -> Result<Vec<u8>, &'static str> {
    if !supported_sound_path(path) {
        return Err("Choose an absolute MP3, WAV, OGG, FLAC, AAC, or M4A file.");
    }
    let file =
        std::fs::File::open(Path::new(path)).map_err(|_| "The audio file could not be opened.")?;
    if !file
        .metadata()
        .map_err(|_| "The audio file could not be read.")?
        .is_file()
    {
        return Err("Choose an audio file, not a folder.");
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOUND_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The audio file could not be read.")?;
    if bytes.len() as u64 > MAX_SOUND_BYTES {
        return Err("Choose an audio file no larger than 5 MiB.");
    }
    Ok(bytes)
}

fn decode(bytes: Vec<u8>) -> Result<Decoder<Cursor<Vec<u8>>>, &'static str> {
    Decoder::try_from(Cursor::new(bytes))
        .map_err(|_| "This file is not supported audio. Choose another file.")
}

pub fn validate(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Ok(());
    }
    let mut source = decode(read_audio(path)?)?;
    source
        .next()
        .ok_or("This audio file contains no playable samples.")?;
    Ok(())
}

#[derive(Default)]
pub struct SoundPreview {
    cancelled: Option<Arc<AtomicBool>>,
}

impl SoundPreview {
    pub fn stop(&mut self) {
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Release);
        }
    }

    pub fn start(&mut self, path: String) -> oneshot::Receiver<Result<(), &'static str>> {
        self.stop();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancelled = Some(cancelled.clone());
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            let result = (|| {
                let source = decode(read_audio(&path)?)?;
                if cancelled.load(Ordering::Acquire) {
                    return Ok(());
                }
                let mut device = DeviceSinkBuilder::open_default_sink()
                    .map_err(|_| "No audio output device is available.")?;
                device.log_on_drop(false);
                let player = Player::connect_new(device.mixer());
                player.append(source);
                let started = Instant::now();
                while !player.empty()
                    && !cancelled.load(Ordering::Acquire)
                    && started.elapsed() < Duration::from_secs(30)
                {
                    std::thread::sleep(Duration::from_millis(25));
                }
                player.stop();
                Ok(())
            })();
            let _ = tx.send(result);
        });
        rx
    }
}

impl Drop for SoundPreview {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_audio_is_rejected_without_an_output_device() {
        assert!(decode(b"not an audio file".to_vec()).is_err());
        assert!(validate("relative.wav").is_err());
        assert!(validate("").is_ok());
    }

    #[test]
    fn stopping_a_preview_signals_its_worker_and_releases_ownership() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut preview = SoundPreview {
            cancelled: Some(cancelled.clone()),
        };
        preview.stop();
        assert!(cancelled.load(Ordering::Acquire));
        assert!(preview.cancelled.is_none());
    }

    #[test]
    fn valid_wave_samples_are_accepted_and_oversize_files_rejected_before_decoding() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.wav");
        let mut wave = b"RIFF".to_vec();
        wave.extend(38_u32.to_le_bytes());
        wave.extend(b"WAVEfmt ");
        wave.extend(16_u32.to_le_bytes());
        wave.extend(1_u16.to_le_bytes());
        wave.extend(1_u16.to_le_bytes());
        wave.extend(8000_u32.to_le_bytes());
        wave.extend(16000_u32.to_le_bytes());
        wave.extend(2_u16.to_le_bytes());
        wave.extend(16_u16.to_le_bytes());
        wave.extend(b"data");
        wave.extend(2_u32.to_le_bytes());
        wave.extend(0_i16.to_le_bytes());
        std::fs::write(&path, wave).unwrap();
        assert!(validate(path.to_str().unwrap()).is_ok());
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_SOUND_BYTES + 1)
            .unwrap();
        assert_eq!(
            validate(path.to_str().unwrap()),
            Err("Choose an audio file no larger than 5 MiB.")
        );
        assert!(validate(directory.path().to_str().unwrap()).is_err());
    }
}
