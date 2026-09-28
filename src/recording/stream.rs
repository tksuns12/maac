use std::cell::UnsafeCell;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use sha2::{Digest, Sha256};

use super::{
    recording_error, CliError, Continuity, Delivered, InputDevice, Latency, Origin,
    RecordingMetadata, BACKEND, MAX_METADATA_BYTES, RATE,
};

pub(super) const CHUNK_FRAMES: usize = 2048;
const SLOTS: usize = 16;

#[derive(Clone, Copy)]
pub(super) struct Chunk {
    pub time: f64,
    pub frames: usize,
    pub samples: [f32; CHUNK_FRAMES],
}

/// One callback producer and one disk consumer. All slots are reserved before
/// AudioQueueStart. The callback never waits, allocates, or performs disk I/O.
pub(super) struct Ring {
    slots: Box<[UnsafeCell<Chunk>]>,
    read: AtomicUsize,
    write: AtomicUsize,
    producing: AtomicBool,
    pub failure: AtomicU32,
    #[cfg(target_os = "macos")]
    pub stopping: AtomicBool,
}

// SAFETY: ownership of each slot crosses only through acquire/release indices.
// The producer guard rejects overlapping callbacks instead of racing slots.
unsafe impl Sync for Ring {}

impl Ring {
    pub fn new() -> Self {
        Self {
            slots: (0..SLOTS)
                .map(|_| {
                    UnsafeCell::new(Chunk {
                        time: 0.0,
                        frames: 0,
                        samples: [0.0; CHUNK_FRAMES],
                    })
                })
                .collect(),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            producing: AtomicBool::new(false),
            failure: AtomicU32::new(0),
            #[cfg(target_os = "macos")]
            stopping: AtomicBool::new(false),
        }
    }

    pub fn fail(&self, reason: u32) {
        let _ = self
            .failure
            .compare_exchange(0, reason, Ordering::Relaxed, Ordering::Relaxed);
    }

    pub fn push(&self, time: f64, samples: &[f32]) -> bool {
        if samples.is_empty() || samples.len() > CHUNK_FRAMES || !time.is_finite() {
            self.fail(1);
            return false;
        }
        if self
            .producing
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.fail(2);
            return false;
        }
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        let accepted = if write.wrapping_sub(read) >= SLOTS {
            self.fail(2);
            false
        } else {
            // SAFETY: this producer owns this free slot until write is released.
            let slot = unsafe { &mut *self.slots[write % SLOTS].get() };
            slot.time = time;
            slot.frames = samples.len();
            slot.samples[..samples.len()].copy_from_slice(samples);
            self.write.store(write.wrapping_add(1), Ordering::Release);
            true
        };
        self.producing.store(false, Ordering::Release);
        accepted
    }

    pub fn pop(&self) -> Option<Chunk> {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: acquire publishes the initialized slot; the producer cannot
        // reuse it until this consumer releases the advanced read index.
        let chunk = unsafe { *self.slots[read % SLOTS].get() };
        self.read.store(read.wrapping_add(1), Ordering::Release);
        Some(chunk)
    }

    pub fn check(&self) -> Result<(), CliError> {
        match self.failure.load(Ordering::Relaxed) {
            0 => Ok(()),
            1 => Err(recording_error(
                "input callback returned invalid frames or timestamp",
            )),
            2 => Err(recording_error(
                "bounded recording callback queue overflowed",
            )),
            4 => Err(recording_error("cannot re-enqueue microphone input buffer")),
            _ => Err(recording_error(
                "microphone input changed or became unavailable",
            )),
        }
    }
}

pub(super) struct WaveSink {
    writer: BufWriter<File>,
    frames: u64,
    target: u64,
    expected_time: Option<f64>,
    hasher: Sha256,
}

impl WaveSink {
    pub fn create(path: &Path, duration: u32) -> Result<Self, CliError> {
        let target = u64::from(duration) * u64::from(RATE);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut writer = BufWriter::new(options.open(path).map_err(io_error)?);
        let bytes = u32::try_from(target * 4)
            .map_err(|_| recording_error("recording data extent overflow"))?;
        writer.write_all(b"RIFF").map_err(io_error)?;
        writer
            .write_all(&(36 + bytes).to_le_bytes())
            .map_err(io_error)?;
        writer.write_all(b"WAVEfmt ").map_err(io_error)?;
        writer.write_all(&16u32.to_le_bytes()).map_err(io_error)?;
        writer.write_all(&3u16.to_le_bytes()).map_err(io_error)?;
        writer.write_all(&1u16.to_le_bytes()).map_err(io_error)?;
        writer.write_all(&RATE.to_le_bytes()).map_err(io_error)?;
        writer
            .write_all(&(RATE * 4).to_le_bytes())
            .map_err(io_error)?;
        writer.write_all(&4u16.to_le_bytes()).map_err(io_error)?;
        writer.write_all(&32u16.to_le_bytes()).map_err(io_error)?;
        writer.write_all(b"data").map_err(io_error)?;
        writer.write_all(&bytes.to_le_bytes()).map_err(io_error)?;
        Ok(Self {
            writer,
            frames: 0,
            target,
            expected_time: None,
            hasher: Sha256::new(),
        })
    }

    pub fn accept(&mut self, chunk: &Chunk) -> Result<(), CliError> {
        if chunk.frames == 0 || chunk.frames > CHUNK_FRAMES || !chunk.time.is_finite() {
            return Err(recording_error("invalid delivered input buffer"));
        }
        if self
            .expected_time
            .is_some_and(|expected| (expected - chunk.time).abs() > 0.000_001)
        {
            return Err(recording_error(
                "detected discontinuity in microphone frame timestamps",
            ));
        }
        self.expected_time = Some(chunk.time + chunk.frames as f64);
        // Validate the complete delivered buffer, even its final unused excess.
        if chunk.samples[..chunk.frames]
            .iter()
            .any(|sample| !sample.is_finite())
        {
            return Err(recording_error("microphone delivered a nonfinite sample"));
        }
        let count = chunk.frames.min((self.target - self.frames) as usize);
        let mut bytes = [0u8; CHUNK_FRAMES * 4];
        for (sample, encoded) in chunk.samples[..count].iter().zip(bytes.chunks_exact_mut(4)) {
            encoded.copy_from_slice(&sample.to_le_bytes());
        }
        self.writer
            .write_all(&bytes[..count * 4])
            .map_err(io_error)?;
        self.hasher.update(&bytes[..count * 4]);
        self.frames += count as u64;
        Ok(())
    }

    pub fn complete(&self) -> bool {
        self.frames == self.target
    }

    pub fn finish(mut self, duration: u32, device: InputDevice) -> Result<(), CliError> {
        if !self.complete() {
            return Err(recording_error(
                "microphone capture ended before the requested frame count",
            ));
        }
        let metadata = RecordingMetadata {
            format: "maac.recording".into(),
            version: 1,
            backend: BACKEND.into(),
            requested_duration_seconds: duration,
            delivered: Delivered {
                encoding: "ieee_f32le".into(),
                rate_hz: RATE,
                channels: 1,
                frames: self.frames,
            },
            input_device: device,
            origin: Origin {
                kind: "first-delivered-frame".into(),
                source_frame: 0,
            },
            latency: Latency {
                measured_input_frames: None,
                compensation_frames: 0,
            },
            continuity: Continuity {
                policy: "abort-on-detected-discontinuity".into(),
                detected_discontinuities: 0,
            },
            data_sha256: format!("sha256:{:x}", self.hasher.finalize()),
        };
        let bytes = serde_json::to_vec(&metadata).map_err(|error| {
            recording_error(format!("cannot encode recording provenance: {error}"))
        })?;
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(recording_error(
                "recording provenance exceeds its bounded envelope",
            ));
        }
        self.writer.write_all(b"maac").map_err(io_error)?;
        self.writer
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .map_err(io_error)?;
        self.writer.write_all(&bytes).map_err(io_error)?;
        if bytes.len() % 2 != 0 {
            self.writer.write_all(&[0]).map_err(io_error)?;
        }
        let length = self.writer.stream_position().map_err(io_error)?;
        self.writer.seek(SeekFrom::Start(4)).map_err(io_error)?;
        self.writer
            .write_all(&((length - 8) as u32).to_le_bytes())
            .map_err(io_error)?;
        self.writer.flush().map_err(io_error)?;
        self.writer.get_ref().sync_all().map_err(io_error)?;
        Ok(())
    }
}

/// The parent checks exact container extent, unique format/data/provenance,
/// finite PCM, strict metadata, and the data-only digest using bounded buffers.
pub(super) fn validate_wave(
    path: &Path,
    duration: u32,
    mut check: impl FnMut() -> Result<(), CliError>,
) -> Result<RecordingMetadata, CliError> {
    let mut file = File::open(path).map_err(io_error)?;
    let length = file.metadata().map_err(io_error)?.len();
    let expected_frames = u64::from(duration) * u64::from(RATE);
    if length > 44 + expected_frames * 4 + 8 + MAX_METADATA_BYTES as u64 + 1 {
        return Err(recording_error("recording WAV exceeds its bounded extent"));
    }
    let mut header = [0u8; 44];
    file.read_exact(&mut header).map_err(io_error)?;
    if &header[..4] != b"RIFF"
        || &header[8..16] != b"WAVEfmt "
        || u64::from(u32::from_le_bytes(header[4..8].try_into().unwrap())) + 8 != length
        || u32::from_le_bytes(header[16..20].try_into().unwrap()) != 16
        || u16::from_le_bytes(header[20..22].try_into().unwrap()) != 3
        || u16::from_le_bytes(header[22..24].try_into().unwrap()) != 1
        || u32::from_le_bytes(header[24..28].try_into().unwrap()) != RATE
        || u32::from_le_bytes(header[28..32].try_into().unwrap()) != RATE * 4
        || u16::from_le_bytes(header[32..34].try_into().unwrap()) != 4
        || u16::from_le_bytes(header[34..36].try_into().unwrap()) != 32
        || &header[36..40] != b"data"
        || u64::from(u32::from_le_bytes(header[40..44].try_into().unwrap())) != expected_frames * 4
    {
        return Err(recording_error(
            "capture child returned inconsistent WAV format or extent",
        ));
    }
    let mut remaining = expected_frames * 4;
    let mut block = [0u8; 64 * 1024];
    let mut hasher = Sha256::new();
    while remaining != 0 {
        check()?;
        let count = remaining.min(block.len() as u64) as usize;
        file.read_exact(&mut block[..count]).map_err(io_error)?;
        if block[..count]
            .chunks_exact(4)
            .any(|sample| !f32::from_le_bytes(sample.try_into().unwrap()).is_finite())
        {
            return Err(recording_error("capture child returned nonfinite PCM"));
        }
        hasher.update(&block[..count]);
        remaining -= count as u64;
    }
    let mut chunk = [0u8; 8];
    file.read_exact(&mut chunk).map_err(io_error)?;
    let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as usize;
    if &chunk[..4] != b"maac"
        || size > MAX_METADATA_BYTES
        || 44 + expected_frames * 4 + 8 + size as u64 + (size % 2) as u64 != length
    {
        return Err(recording_error(
            "capture child returned missing, duplicate, or oversized recording provenance",
        ));
    }
    let mut bytes = vec![0; size];
    file.read_exact(&mut bytes).map_err(io_error)?;
    if !size.is_multiple_of(2) {
        let mut pad = [0];
        file.read_exact(&mut pad).map_err(io_error)?;
        if pad != [0] {
            return Err(recording_error("invalid recording provenance padding"));
        }
    }
    let metadata: RecordingMetadata = serde_json::from_slice(&bytes)
        .map_err(|error| recording_error(format!("invalid recording provenance: {error}")))?;
    if metadata.format != "maac.recording"
        || metadata.version != 1
        || metadata.backend != BACKEND
        || metadata.requested_duration_seconds != duration
        || metadata.delivered.encoding != "ieee_f32le"
        || metadata.delivered.rate_hz != RATE
        || metadata.delivered.channels != 1
        || metadata.delivered.frames != expected_frames
        || metadata.input_device.selection != "system-default"
        || metadata.input_device.uid.is_empty()
        || metadata.input_device.uid.len() > 4096
        || metadata
            .input_device
            .hardware_rate_hz
            .is_some_and(|rate| !rate.is_finite() || rate <= 0.0)
        || metadata.origin.kind != "first-delivered-frame"
        || metadata.origin.source_frame != 0
        || metadata.latency.measured_input_frames.is_some()
        || metadata.latency.compensation_frames != 0
        || metadata.continuity.policy != "abort-on-detected-discontinuity"
        || metadata.continuity.detected_discontinuities != 0
        || metadata.data_sha256 != format!("sha256:{:x}", hasher.finalize())
    {
        return Err(recording_error(
            "recording provenance differs from the checked PCM or capture contract",
        ));
    }
    Ok(metadata)
}

fn io_error(error: std::io::Error) -> CliError {
    CliError::new(
        "E_IO",
        format!("cannot access private recording WAV: {error}"),
    )
}
