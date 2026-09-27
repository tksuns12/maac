//! Bounded WAV import into MaaC's exact interleaved float32 audio-asset format.
//!
//! The input is snapshotted to temporary storage and hashed before decoding so
//! the recorded source identity is the identity of the bytes that were read.
//! Only the selected crop is retained in memory as a MaaC asset.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

use hound::{SampleFormat, WavSpec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::bundle::{sha256_digest, SourceBundle, MAX_BUNDLE_FILE_BYTES};
use crate::bundle_fs;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub const MEDIA_IMPORT_FORMAT: &str = "maac.media-import";
pub const MEDIA_IMPORT_VERSION: u32 = 1;
pub const MEDIA_IMPORT_RETAINED_VERSION: u32 = 2;
pub const MEDIA_IMPORT_DECODER_ID: &str = "maac.wav-import/1";
pub const MEDIA_IMPORT_CONVERSION_ID: &str = "maac.pcm-f32le-conversion/1";
pub const MAX_MEDIA_IMPORT_INPUT_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_MEDIA_IMPORT_DECODED_BYTES: u64 = 1024 * 1024 * 1024;
const SNAPSHOT_BUFFER_BYTES: usize = 64 * 1024;
const MAX_MEDIA_IMPORT_MANIFEST_BYTES: u64 = 16 * 1024;
const CORE_AUDIO_FORMAT: &str = "pcm_f32le_interleaved/1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WavImportRange {
    start_frame: u64,
    end_frame: u64,
}

impl WavImportRange {
    pub const fn new(start_frame: u64, end_frame: u64) -> Self {
        Self {
            start_frame,
            end_frame,
        }
    }

    pub const fn start_frame(self) -> u64 {
        self.start_frame
    }

    pub const fn end_frame(self) -> u64 {
        self.end_frame
    }
}

#[derive(Debug)]
pub struct MediaImportError {
    code: &'static str,
    message: String,
}

impl MediaImportError {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for MediaImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for MediaImportError {}

/// The exact source identity, crop, native PCM bytes, and ordinary editable
/// project produced by one WAV import.
#[derive(Clone, Debug)]
pub struct ImportedWav {
    input_name: String,
    input_bytes: u64,
    source_hash: String,
    source_encoding: String,
    source_rate_hz: u32,
    source_channels: u16,
    source_frames: u32,
    range: WavImportRange,
    pcm_bytes: Vec<u8>,
    pcm_hash: String,
    retained_snapshot: Option<Arc<NamedTempFile>>,
}

impl PartialEq for ImportedWav {
    fn eq(&self, other: &Self) -> bool {
        self.input_name == other.input_name
            && self.input_bytes == other.input_bytes
            && self.source_hash == other.source_hash
            && self.source_encoding == other.source_encoding
            && self.source_rate_hz == other.source_rate_hz
            && self.source_channels == other.source_channels
            && self.source_frames == other.source_frames
            && self.range == other.range
            && self.pcm_bytes == other.pcm_bytes
            && self.pcm_hash == other.pcm_hash
            && self.retained_snapshot.is_some() == other.retained_snapshot.is_some()
    }
}

impl Eq for ImportedWav {}

impl ImportedWav {
    pub fn input_name(&self) -> &str {
        &self.input_name
    }

    pub fn input_bytes(&self) -> u64 {
        self.input_bytes
    }

    pub fn source_hash(&self) -> &str {
        &self.source_hash
    }

    pub fn source_encoding(&self) -> &str {
        &self.source_encoding
    }

    pub fn source_rate_hz(&self) -> u32 {
        self.source_rate_hz
    }

    pub fn source_channels(&self) -> u16 {
        self.source_channels
    }

    pub fn source_frames(&self) -> u32 {
        self.source_frames
    }

    pub fn range(&self) -> WavImportRange {
        self.range
    }

    pub fn channels(&self) -> u8 {
        self.source_channels as u8
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.source_rate_hz
    }

    pub fn frames(&self) -> u64 {
        self.range.end_frame - self.range.start_frame
    }

    pub fn pcm_bytes(&self) -> &[u8] {
        &self.pcm_bytes
    }

    pub fn pcm_hash(&self) -> &str {
        &self.pcm_hash
    }

    pub fn manifest_version(&self) -> u32 {
        if self.retained_snapshot.is_some() {
            MEDIA_IMPORT_RETAINED_VERSION
        } else {
            MEDIA_IMPORT_VERSION
        }
    }

    pub fn copy_original_to(&self, destination: &Path) -> Result<(), MediaImportError> {
        let snapshot = self.retained_snapshot.as_ref().ok_or_else(|| {
            import_error(
                "E_INTERNAL",
                "import did not retain its original WAV snapshot",
            )
        })?;
        let mut source = snapshot.reopen().map_err(|error| {
            import_error(
                "E_IO",
                format!("cannot reopen retained WAV snapshot: {error}"),
            )
        })?;
        let mut output = File::create(destination)
            .map_err(|error| import_error("E_IO", format!("cannot stage original.wav: {error}")))?;
        let bytes = io::copy(&mut source, &mut output).map_err(|error| {
            import_error(
                "E_IO",
                format!("cannot copy retained WAV snapshot: {error}"),
            )
        })?;
        if bytes != self.input_bytes {
            return Err(import_error(
                "E_IO",
                "retained WAV snapshot changed during copying",
            ));
        }
        Ok(())
    }

    /// Build the ordinary source bundle that is written by the CLI and can
    /// subsequently pass through the existing check/build/compile pipeline.
    pub fn source_bundle(&self) -> SourceBundle {
        let source = source_text(
            self.frames(),
            self.source_rate_hz,
            self.source_channels,
            &self.pcm_hash,
        );
        SourceBundle {
            entry: "main.maac".into(),
            sources: [("main.maac".into(), source)].into(),
            assets: [("media.pcm".into(), self.pcm_bytes.clone())].into(),
        }
    }

    pub fn manifest_json(&self) -> Result<Vec<u8>, MediaImportError> {
        import_manifest_json(
            &self.input_name,
            self.input_bytes,
            &self.source_hash,
            &self.source_encoding,
            self.source_rate_hz,
            self.source_channels,
            self.source_frames,
            self.range,
            &self.pcm_hash,
            self.manifest_version(),
        )
    }
}

fn source_text(frames: u64, rate_hz: u32, channels: u16, pcm_hash: &str) -> String {
    format!(
        "maac 1;\n\
             project imported_media {{\n\
               score = [0q, {}/{}q];\n\
               tail = 0s;\n\
               rate = 48000Hz;\n\
               tempo = &tempo;\n\
               meter = &meter;\n\
               output = &clip:out;\n\
             }}\n\
             tempo tempo {{ points = [(0q, 60bpm, step)]; }}\n\
             meter meter {{ points = [(0q, 4, 4)]; }}\n\
             asset media {{\n\
               kind = audio;\n\
               path = \"media.pcm\";\n\
               hash = \"{}\";\n\
               format = \"{}\";\n\
               rate = {}Hz;\n\
               channels = {};\n\
               frames = {};\n\
             }}\n\
             audio clip {{\n\
               asset = &media;\n\
               at = 0q;\n\
               source = [0frame, {}frame];\n\
               mode = rate;\n\
             }}\n",
        frames, rate_hz, pcm_hash, CORE_AUDIO_FORMAT, rate_hz, channels, frames, frames,
    )
}

#[allow(clippy::too_many_arguments)]
fn import_manifest_json(
    input_name: &str,
    input_bytes: u64,
    source_hash: &str,
    source_encoding: &str,
    source_rate_hz: u32,
    source_channels: u16,
    source_frames: u32,
    range: WavImportRange,
    pcm_hash: &str,
    manifest_version: u32,
) -> Result<Vec<u8>, MediaImportError> {
    let manifest = MediaImportManifest {
        format: MEDIA_IMPORT_FORMAT,
        version: manifest_version,
        decoder: MEDIA_IMPORT_DECODER_ID,
        conversion: MEDIA_IMPORT_CONVERSION_ID,
        source: MediaImportSource {
            name: input_name,
            bytes: input_bytes,
            sha256: source_hash,
            container: "riff_wave",
            encoding: source_encoding,
            rate_hz: source_rate_hz,
            channels: source_channels,
            frames: source_frames,
        },
        selection: MediaImportSelection {
            start_frame: range.start_frame,
            end_frame: range.end_frame,
        },
        output: MediaImportOutput {
            path: "media.pcm",
            format: CORE_AUDIO_FORMAT,
            rate_hz: source_rate_hz,
            channels: source_channels,
            frames: range.end_frame - range.start_frame,
            sha256: pcm_hash,
        },
        original: (manifest_version == MEDIA_IMPORT_RETAINED_VERSION).then_some(
            MediaImportOriginal {
                path: "original.wav",
                bytes: input_bytes,
                sha256: source_hash,
            },
        ),
    };
    serde_json::to_vec_pretty(&manifest).map_err(|error| {
        import_error(
            "E_INTERNAL",
            format!("cannot encode import manifest: {error}"),
        )
    })
}

/// Snapshot, identify, and convert one WAV file into the supported native PCM
/// asset. With no `range`, the complete nonempty WAV is imported.
pub fn import_wav_file(
    input_path: &Path,
    range: Option<WavImportRange>,
) -> Result<ImportedWav, MediaImportError> {
    import_wav_file_with_retention(input_path, range, false)
}

pub fn import_wav_file_with_retention(
    input_path: &Path,
    range: Option<WavImportRange>,
    retain_original: bool,
) -> Result<ImportedWav, MediaImportError> {
    let input = open_import_input(input_path).map_err(|error| {
        import_error(
            "E_IO",
            format!("cannot open {}: {error}", input_path.display()),
        )
    })?;
    let input_name = input_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "input.wav".into());
    import_wav_open_file(input, input_name, range, retain_original)
}

fn import_wav_open_file(
    input: File,
    input_name: String,
    range: Option<WavImportRange>,
    retain_original: bool,
) -> Result<ImportedWav, MediaImportError> {
    let (snapshot, input_bytes, source_hash) = snapshot_and_hash(input)?;

    let layout = inspect_wav(&snapshot, input_bytes)?;
    let spec = layout.spec;
    let source_encoding = source_encoding(spec.sample_format, spec.bits_per_sample)?;
    let source_frames = layout.data_bytes / u32::from(layout.block_align);
    let range = range.unwrap_or(WavImportRange::new(0, u64::from(source_frames)));
    if range.start_frame >= range.end_frame || range.end_frame > u64::from(source_frames) {
        return Err(import_error(
            "E_RANGE",
            format!("crop must satisfy 0 <= start < end <= {source_frames} frames"),
        ));
    }

    let frames = range.end_frame - range.start_frame;
    let sample_count = frames
        .checked_mul(u64::from(spec.channels))
        .ok_or_else(|| import_error("E_RESOURCE_LIMIT", "crop sample count overflows"))?;
    let output_bytes = sample_count
        .checked_mul(4)
        .and_then(|size| usize::try_from(size).ok())
        .ok_or_else(|| import_error("E_RESOURCE_LIMIT", "crop byte count overflows"))?;
    if output_bytes > MAX_BUNDLE_FILE_BYTES {
        return Err(import_error(
            "E_RESOURCE_LIMIT",
            format!(
                "imported PCM crop exceeds the existing {}-byte asset limit",
                MAX_BUNDLE_FILE_BYTES
            ),
        ));
    }

    let crop_offset = layout.data_offset + range.start_frame * u64::from(layout.block_align);
    let snapshot_reader = snapshot
        .reopen()
        .map_err(|error| import_error("E_IO", format!("cannot reopen WAV snapshot: {error}")))?;
    let mut reader = BufReader::new(snapshot_reader);
    reader
        .seek(SeekFrom::Start(crop_offset))
        .map_err(|error| import_error("E_WAV", format!("cannot seek to crop: {error}")))?;
    let mut pcm_bytes = Vec::new();
    pcm_bytes
        .try_reserve_exact(output_bytes)
        .map_err(|_| import_error("E_RESOURCE_LIMIT", "cannot allocate selected PCM crop"))?;
    let sample_bytes = usize::from(spec.bits_per_sample / 8);
    let mut bytes = [0u8; 4];
    for _ in 0..sample_count {
        reader
            .read_exact(&mut bytes[..sample_bytes])
            .map_err(|error| {
                import_error(
                    "E_WAV",
                    format!("WAV data ended before the selected crop: {error}"),
                )
            })?;
        let converted = convert_sample(spec.sample_format, sample_bytes, bytes)?;
        pcm_bytes.extend_from_slice(&converted.to_le_bytes());
    }

    let pcm_hash = sha256_digest(&pcm_bytes);
    Ok(ImportedWav {
        input_name,
        input_bytes,
        source_hash,
        source_encoding,
        source_rate_hz: spec.sample_rate,
        source_channels: spec.channels,
        source_frames,
        range,
        pcm_bytes,
        pcm_hash,
        retained_snapshot: retain_original.then(|| Arc::new(snapshot)),
    })
}

fn convert_sample(
    format: SampleFormat,
    sample_bytes: usize,
    bytes: [u8; 4],
) -> Result<f32, MediaImportError> {
    Ok(match (format, sample_bytes) {
        (SampleFormat::Int, 2) => {
            f64::from(i16::from_le_bytes([bytes[0], bytes[1]])) as f32 / 32_768.0
        }
        (SampleFormat::Int, 3) => {
            let sign = if bytes[2] & 0x80 == 0 { 0 } else { 0xff };
            let sample = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], sign]);
            (f64::from(sample) / 8_388_608.0) as f32
        }
        (SampleFormat::Int, 4) => (f64::from(i32::from_le_bytes(bytes)) / 2_147_483_648.0) as f32,
        (SampleFormat::Float, 4) => {
            let sample = f32::from_le_bytes(bytes);
            if !sample.is_finite() {
                return Err(import_error(
                    "E_ASSET",
                    "selected WAV crop contains a nonfinite float sample",
                ));
            }
            sample
        }
        _ => unreachable!("source_encoding checked the sample representation"),
    })
}

/// An imported WAV whose decoded PCM is held in private temporary storage.
/// Cloning keeps both snapshots alive; the decoded crop is never materialized
/// as a single allocation.
#[derive(Clone, Debug)]
pub struct DiskImportedWav {
    input_name: String,
    input_bytes: u64,
    source_hash: String,
    source_encoding: String,
    source_rate_hz: u32,
    source_channels: u16,
    source_frames: u32,
    range: WavImportRange,
    pcm_bytes_len: u64,
    pcm_hash: String,
    pcm_snapshot: Arc<NamedTempFile>,
    retained_snapshot: Option<Arc<NamedTempFile>>,
}

impl DiskImportedWav {
    pub fn input_bytes(&self) -> u64 {
        self.input_bytes
    }

    pub fn source_hash(&self) -> &str {
        &self.source_hash
    }

    pub fn pcm_hash(&self) -> &str {
        &self.pcm_hash
    }

    pub fn pcm_bytes_len(&self) -> u64 {
        self.pcm_bytes_len
    }

    pub fn frames(&self) -> u64 {
        self.range.end_frame - self.range.start_frame
    }

    pub fn manifest_version(&self) -> u32 {
        if self.retained_snapshot.is_some() {
            MEDIA_IMPORT_RETAINED_VERSION
        } else {
            MEDIA_IMPORT_VERSION
        }
    }

    pub fn source_text(&self) -> String {
        source_text(
            self.frames(),
            self.source_rate_hz,
            self.source_channels,
            &self.pcm_hash,
        )
    }

    pub fn manifest_json(&self) -> Result<Vec<u8>, MediaImportError> {
        import_manifest_json(
            &self.input_name,
            self.input_bytes,
            &self.source_hash,
            &self.source_encoding,
            self.source_rate_hz,
            self.source_channels,
            self.source_frames,
            self.range,
            &self.pcm_hash,
            self.manifest_version(),
        )
    }

    pub fn stage_pcm_to(&self, destination: &Path) -> Result<(), MediaImportError> {
        stage_snapshot(
            &self.pcm_snapshot,
            destination,
            self.pcm_bytes_len,
            &self.pcm_hash,
            "media.pcm",
        )
    }

    pub fn stage_retained_original_to(&self, destination: &Path) -> Result<(), MediaImportError> {
        let snapshot = self
            .retained_snapshot
            .as_ref()
            .ok_or_else(|| import_error("E_INTERNAL", "import has no private original snapshot"))?;
        stage_snapshot(
            snapshot,
            destination,
            self.input_bytes,
            &self.source_hash,
            "original.wav",
        )
    }

    pub(crate) fn pcm_snapshot(&self) -> &NamedTempFile {
        &self.pcm_snapshot
    }
}

/// Opt-in file-backed WAV import for selected crops up to 1 GiB of decoded
/// interleaved float32 PCM. Its manifest and source text are identical to the
/// in-memory importer for the same crop.
pub fn import_wav_file_disk_with_retention(
    input_path: &Path,
    range: Option<WavImportRange>,
    retain_original: bool,
) -> Result<DiskImportedWav, MediaImportError> {
    let input = open_import_input(input_path).map_err(|error| {
        import_error(
            "E_IO",
            format!("cannot open {}: {error}", input_path.display()),
        )
    })?;
    let input_name = input_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "input.wav".into());
    import_wav_open_file_disk(input, input_name, range, retain_original)
}

fn import_wav_open_file_disk(
    input: File,
    input_name: String,
    range: Option<WavImportRange>,
    retain_original: bool,
) -> Result<DiskImportedWav, MediaImportError> {
    let (snapshot, input_bytes, source_hash) = snapshot_and_hash(input)?;
    let layout = inspect_wav(&snapshot, input_bytes)?;
    let spec = layout.spec;
    let source_encoding = source_encoding(spec.sample_format, spec.bits_per_sample)?;
    let source_frames = layout.data_bytes / u32::from(layout.block_align);
    let range = range.unwrap_or(WavImportRange::new(0, u64::from(source_frames)));
    if range.start_frame >= range.end_frame || range.end_frame > u64::from(source_frames) {
        return Err(import_error(
            "E_RANGE",
            format!("crop must satisfy 0 <= start < end <= {source_frames} frames"),
        ));
    }
    let frames = range.end_frame - range.start_frame;
    let sample_count = frames
        .checked_mul(u64::from(spec.channels))
        .ok_or_else(|| import_error("E_RESOURCE_LIMIT", "crop sample count overflows"))?;
    let pcm_bytes_len = sample_count
        .checked_mul(4)
        .ok_or_else(|| import_error("E_RESOURCE_LIMIT", "crop byte count overflows"))?;
    if pcm_bytes_len > MAX_MEDIA_IMPORT_DECODED_BYTES {
        return Err(import_error(
            "E_RESOURCE_LIMIT",
            format!("imported PCM crop exceeds {MAX_MEDIA_IMPORT_DECODED_BYTES} bytes"),
        ));
    }

    let crop_offset = layout.data_offset + range.start_frame * u64::from(layout.block_align);
    let snapshot_reader = snapshot
        .reopen()
        .map_err(|error| import_error("E_IO", format!("cannot reopen WAV snapshot: {error}")))?;
    let mut reader = BufReader::new(snapshot_reader);
    reader
        .seek(SeekFrom::Start(crop_offset))
        .map_err(|error| import_error("E_WAV", format!("cannot seek to crop: {error}")))?;
    let mut pcm_snapshot = NamedTempFile::new()
        .map_err(|error| import_error("E_IO", format!("cannot create PCM snapshot: {error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = Vec::with_capacity(SNAPSHOT_BUFFER_BYTES);
    let sample_bytes = usize::from(spec.bits_per_sample / 8);
    let mut bytes = [0u8; 4];
    for _ in 0..sample_count {
        reader
            .read_exact(&mut bytes[..sample_bytes])
            .map_err(|error| {
                import_error(
                    "E_WAV",
                    format!("WAV data ended before the selected crop: {error}"),
                )
            })?;
        let converted = convert_sample(spec.sample_format, sample_bytes, bytes)?;
        buffer.extend_from_slice(&converted.to_le_bytes());
        if buffer.len() == SNAPSHOT_BUFFER_BYTES {
            pcm_snapshot.write_all(&buffer).map_err(|error| {
                import_error("E_IO", format!("cannot write PCM snapshot: {error}"))
            })?;
            hasher.update(&buffer);
            buffer.clear();
        }
    }
    if !buffer.is_empty() {
        pcm_snapshot
            .write_all(&buffer)
            .map_err(|error| import_error("E_IO", format!("cannot write PCM snapshot: {error}")))?;
        hasher.update(&buffer);
    }
    let pcm_hash = format!("sha256:{:x}", hasher.finalize());
    Ok(DiskImportedWav {
        input_name,
        input_bytes,
        source_hash,
        source_encoding,
        source_rate_hz: spec.sample_rate,
        source_channels: spec.channels,
        source_frames,
        range,
        pcm_bytes_len,
        pcm_hash,
        pcm_snapshot: Arc::new(pcm_snapshot),
        retained_snapshot: retain_original.then(|| Arc::new(snapshot)),
    })
}

fn stage_snapshot(
    snapshot: &NamedTempFile,
    destination: &Path,
    expected_bytes: u64,
    expected_hash: &str,
    name: &str,
) -> Result<(), MediaImportError> {
    let mut input = snapshot
        .reopen()
        .map_err(|error| import_error("E_IO", format!("cannot reopen {name} snapshot: {error}")))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| import_error("E_IO", format!("cannot create {name}: {error}")))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut block = [0u8; SNAPSHOT_BUFFER_BYTES];
    loop {
        let n = input.read(&mut block).map_err(|error| {
            import_error("E_IO", format!("cannot read {name} snapshot: {error}"))
        })?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| import_error("E_RESOURCE_LIMIT", "snapshot size overflow"))?;
        if total > expected_bytes {
            return Err(import_error(
                "E_IMPORT_MISMATCH",
                format!("{name} snapshot grew"),
            ));
        }
        output
            .write_all(&block[..n])
            .map_err(|error| import_error("E_IO", format!("cannot write {name}: {error}")))?;
        hasher.update(&block[..n]);
    }
    if total != expected_bytes || format!("sha256:{:x}", hasher.finalize()) != expected_hash {
        return Err(import_error(
            "E_IMPORT_MISMATCH",
            format!("{name} snapshot changed"),
        ));
    }
    Ok(())
}

fn source_encoding(format: SampleFormat, bits_per_sample: u16) -> Result<String, MediaImportError> {
    match (format, bits_per_sample) {
        (SampleFormat::Int, 16) => Ok("pcm_s16le".into()),
        (SampleFormat::Int, 24) => Ok("pcm_s24le".into()),
        (SampleFormat::Int, 32) => Ok("pcm_s32le".into()),
        (SampleFormat::Float, 32) => Ok("ieee_f32le".into()),
        _ => Err(import_error(
            "E_CAPABILITY",
            format!("unsupported WAV encoding: {format:?} with {bits_per_sample} bits per sample"),
        )),
    }
}

fn snapshot_and_hash(mut input: File) -> Result<(NamedTempFile, u64, String), MediaImportError> {
    let metadata = input
        .metadata()
        .map_err(|error| import_error("E_IO", format!("cannot inspect WAV input: {error}")))?;
    if !metadata.is_file() {
        return Err(import_error(
            "E_REFERENCE",
            "WAV input is not a regular file",
        ));
    }
    if metadata.len() > MAX_MEDIA_IMPORT_INPUT_BYTES {
        return Err(import_error(
            "E_RESOURCE_LIMIT",
            format!("WAV input exceeds {MAX_MEDIA_IMPORT_INPUT_BYTES} bytes"),
        ));
    }

    let mut snapshot = NamedTempFile::new()
        .map_err(|error| import_error("E_IO", format!("cannot create WAV snapshot: {error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; SNAPSHOT_BUFFER_BYTES];
    let mut total = 0u64;
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|error| import_error("E_IO", format!("cannot snapshot WAV input: {error}")))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| import_error("E_RESOURCE_LIMIT", "WAV input size overflows"))?;
        if total > MAX_MEDIA_IMPORT_INPUT_BYTES {
            return Err(import_error(
                "E_RESOURCE_LIMIT",
                format!("WAV input exceeds {MAX_MEDIA_IMPORT_INPUT_BYTES} bytes"),
            ));
        }
        snapshot
            .write_all(&buffer[..count])
            .map_err(|error| import_error("E_IO", format!("cannot write WAV snapshot: {error}")))?;
        hasher.update(&buffer[..count]);
    }
    let source_hash = format!("sha256:{:x}", hasher.finalize());
    Ok((snapshot, total, source_hash))
}

#[cfg(unix)]
fn open_import_input(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_import_input(path: &Path) -> io::Result<File> {
    File::open(path)
}

struct WavLayout {
    spec: WavSpec,
    block_align: u16,
    data_offset: u64,
    data_bytes: u32,
}

/// Inspect every chunk within the declared RIFF extent. This is the sole
/// authority for both the sample format and data location; accepting another
/// fmt/data chunk would make the interpretation of the source ambiguous.
fn inspect_wav(snapshot: &NamedTempFile, input_bytes: u64) -> Result<WavLayout, MediaImportError> {
    let file = snapshot
        .reopen()
        .map_err(|error| import_error("E_IO", format!("cannot reopen WAV snapshot: {error}")))?;
    let mut reader = BufReader::new(file);
    let mut riff_header = [0u8; 12];
    reader
        .read_exact(&mut riff_header)
        .map_err(|error| import_error("E_WAV", format!("cannot inspect WAV layout: {error}")))?;
    if &riff_header[..4] != b"RIFF" || &riff_header[8..] != b"WAVE" {
        return Err(import_error("E_WAV", "unsupported WAV container"));
    }
    let riff_bytes = u64::from(u32::from_le_bytes(riff_header[4..8].try_into().unwrap()));
    let riff_end = 8 + riff_bytes;
    if riff_bytes < 4 || riff_end > input_bytes {
        return Err(import_error("E_WAV", "WAV RIFF size exceeds the input"));
    }

    let mut position = 12u64;
    let mut format = None;
    let mut data = None;
    while position < riff_end {
        if riff_end - position < 8 {
            return Err(import_error("E_WAV", "truncated WAV chunk header"));
        }
        let mut chunk_header = [0u8; 8];
        reader.read_exact(&mut chunk_header).map_err(|error| {
            import_error("E_WAV", format!("cannot inspect WAV layout: {error}"))
        })?;
        let chunk_bytes = u64::from(u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()));
        let payload = position + 8;
        let payload_end = payload + chunk_bytes;
        // Some writers omit the pad after the final odd-sized data chunk.
        // There is no following chunk to misalign in that one terminal case.
        let next = if &chunk_header[..4] == b"data" && payload_end == riff_end {
            payload_end
        } else {
            payload_end + (chunk_bytes & 1)
        };
        if next > riff_end {
            return Err(import_error("E_WAV", "WAV chunk exceeds the RIFF boundary"));
        }
        match &chunk_header[..4] {
            b"fmt " => {
                if format.is_some() {
                    return Err(import_error("E_WAV", "duplicate WAV format chunk"));
                }
                format = Some(read_wav_format(&mut reader, chunk_bytes)?);
            }
            b"data" => {
                if data.is_some() {
                    return Err(import_error("E_WAV", "duplicate WAV data chunk"));
                }
                if format.is_none() {
                    return Err(import_error("E_WAV", "WAV data precedes format chunk"));
                }
                data = Some((payload, chunk_bytes as u32));
            }
            _ => {}
        }
        reader.seek(SeekFrom::Start(next)).map_err(|error| {
            import_error("E_WAV", format!("cannot inspect WAV layout: {error}"))
        })?;
        position = next;
    }

    let (spec, block_align) =
        format.ok_or_else(|| import_error("E_WAV", "missing WAV format chunk"))?;
    let (data_offset, data_bytes) =
        data.ok_or_else(|| import_error("E_WAV", "missing WAV data chunk"))?;
    if data_bytes % u32::from(block_align) != 0 {
        return Err(import_error(
            "E_WAV",
            "WAV data does not contain complete frames",
        ));
    }
    Ok(WavLayout {
        spec,
        block_align,
        data_offset,
        data_bytes,
    })
}

fn read_wav_format(
    reader: &mut impl Read,
    chunk_bytes: u64,
) -> Result<(WavSpec, u16), MediaImportError> {
    if chunk_bytes < 16 {
        return Err(import_error("E_WAV", "WAV format chunk is too short"));
    }
    let mut header = [0u8; 16];
    reader.read_exact(&mut header).map_err(|error| {
        import_error("E_WAV", format!("cannot inspect WAV format chunk: {error}"))
    })?;
    let tag = u16::from_le_bytes([header[0], header[1]]);
    let channels = u16::from_le_bytes([header[2], header[3]]);
    let sample_rate = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let byte_rate = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let block_align = u16::from_le_bytes([header[12], header[13]]);
    let container_bits = u16::from_le_bytes([header[14], header[15]]);
    if !(1..=2).contains(&channels) {
        return Err(import_error(
            "E_CAPABILITY",
            "WAV import supports mono or stereo sources only",
        ));
    }
    if sample_rate == 0
        || block_align == 0
        || sample_rate.checked_mul(u32::from(block_align)) != Some(byte_rate)
    {
        return Err(import_error("E_WAV", "inconsistent WAV format fields"));
    }
    let (sample_format, bits_per_sample) = match tag {
        1 if matches!(chunk_bytes, 16 | 18 | 40) => (SampleFormat::Int, container_bits),
        3 if matches!(chunk_bytes, 16 | 18) => (SampleFormat::Float, container_bits),
        0xfffe if chunk_bytes == 40 => {
            let mut extension = [0u8; 24];
            reader.read_exact(&mut extension).map_err(|error| {
                import_error(
                    "E_WAV",
                    format!("cannot inspect WAV format extension: {error}"),
                )
            })?;
            if u16::from_le_bytes([extension[0], extension[1]]) != 22 {
                return Err(import_error("E_WAV", "invalid extensible WAV format size"));
            }
            let valid_bits = u16::from_le_bytes([extension[2], extension[3]]);
            let subtype = &extension[8..24];
            let guid_tail = [0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71];
            if subtype[4..] != guid_tail {
                return Err(import_error(
                    "E_CAPABILITY",
                    "unsupported extensible WAV subtype",
                ));
            }
            let sample_format = match subtype[..4] {
                [1, 0, 0, 0] => SampleFormat::Int,
                [3, 0, 0, 0] => SampleFormat::Float,
                _ => {
                    return Err(import_error(
                        "E_CAPABILITY",
                        "unsupported extensible WAV subtype",
                    ))
                }
            };
            (sample_format, valid_bits)
        }
        1 | 3 | 0xfffe => return Err(import_error("E_WAV", "unexpected WAV format chunk size")),
        _ => return Err(import_error("E_CAPABILITY", "unsupported WAV format tag")),
    };
    source_encoding(sample_format, bits_per_sample)?;
    let expected_align = u32::from(channels) * u32::from(bits_per_sample / 8);
    if container_bits != bits_per_sample || u32::from(block_align) != expected_align {
        return Err(import_error(
            "E_CAPABILITY",
            "padded WAV sample containers are not supported; samples must be packed",
        ));
    }
    Ok((
        WavSpec {
            channels,
            sample_rate,
            bits_per_sample,
            sample_format,
        },
        block_align,
    ))
}

#[derive(Serialize)]
struct MediaImportManifest<'a> {
    format: &'static str,
    version: u32,
    decoder: &'static str,
    conversion: &'static str,
    source: MediaImportSource<'a>,
    selection: MediaImportSelection,
    output: MediaImportOutput<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original: Option<MediaImportOriginal<'a>>,
}

#[derive(Serialize)]
struct MediaImportOriginal<'a> {
    path: &'static str,
    bytes: u64,
    sha256: &'a str,
}

#[derive(Serialize)]
struct MediaImportSource<'a> {
    name: &'a str,
    bytes: u64,
    sha256: &'a str,
    container: &'static str,
    encoding: &'a str,
    rate_hz: u32,
    channels: u16,
    frames: u32,
}

#[derive(Serialize)]
struct MediaImportSelection {
    start_frame: u64,
    end_frame: u64,
}

#[derive(Serialize)]
struct MediaImportOutput<'a> {
    path: &'static str,
    format: &'static str,
    rate_hz: u32,
    channels: u16,
    frames: u64,
    sha256: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedManifest {
    format: String,
    version: u32,
    decoder: String,
    conversion: String,
    source: RetainedSource,
    selection: RetainedSelection,
    output: RetainedOutput,
    #[serde(default)]
    original: Option<RetainedOriginal>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedSource {
    name: String,
    bytes: u64,
    sha256: String,
    container: String,
    encoding: String,
    rate_hz: u32,
    channels: u16,
    frames: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedSelection {
    start_frame: u64,
    end_frame: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedOutput {
    path: String,
    format: String,
    rate_hz: u32,
    channels: u16,
    frames: u64,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedOriginal {
    path: String,
    bytes: u64,
    sha256: String,
}

/// Strict, bounded import metadata retained before the potentially large WAV
/// is decoded. The exact JSON bytes remain available for archive staging.
pub(crate) struct RetainedImportPreflight {
    json: Vec<u8>,
    manifest: RetainedManifest,
}

impl RetainedImportPreflight {
    pub(crate) fn json(&self) -> &[u8] {
        &self.json
    }

    pub(crate) fn original_bytes(&self) -> u64 {
        self.manifest.source.bytes
    }

    pub(crate) fn original_hash(&self) -> &str {
        &self.manifest.source.sha256
    }

    pub(crate) fn output_hash(&self) -> &str {
        &self.manifest.output.sha256
    }
}

fn read_import_manifest(
    root: &bundle_fs::ProjectRoot,
) -> Result<(Vec<u8>, RetainedManifest), MediaImportError> {
    let json = read_member(root, "import.json", MAX_MEDIA_IMPORT_MANIFEST_BYTES)?;
    let manifest: RetainedManifest = serde_json::from_slice(&json).map_err(|error| {
        import_error("E_IMPORT_MANIFEST", format!("invalid import.json: {error}"))
    })?;
    Ok((json, manifest))
}

fn validate_import_manifest(manifest: &RetainedManifest) -> Result<(), MediaImportError> {
    let selected_frames = manifest
        .selection
        .end_frame
        .checked_sub(manifest.selection.start_frame);
    let output_bytes = selected_frames
        .and_then(|frames| frames.checked_mul(u64::from(manifest.source.channels)))
        .and_then(|samples| samples.checked_mul(4));
    if manifest.format != MEDIA_IMPORT_FORMAT
        || manifest.decoder != MEDIA_IMPORT_DECODER_ID
        || manifest.conversion != MEDIA_IMPORT_CONVERSION_ID
        || manifest.source.container != "riff_wave"
        || manifest.output.path != "media.pcm"
        || manifest.output.format != CORE_AUDIO_FORMAT
        || manifest.source.bytes > MAX_MEDIA_IMPORT_INPUT_BYTES
        || manifest.source.bytes < 44
        || manifest.source.rate_hz == 0
        || !(1..=2).contains(&manifest.source.channels)
        || !matches!(
            manifest.source.encoding.as_str(),
            "pcm_s16le" | "pcm_s24le" | "pcm_s32le" | "ieee_f32le"
        )
        || manifest.source.frames == 0
        || manifest.selection.start_frame >= manifest.selection.end_frame
        || manifest.selection.end_frame > u64::from(manifest.source.frames)
        || output_bytes.is_none_or(|bytes| bytes > MAX_MEDIA_IMPORT_DECODED_BYTES)
        || !valid_hash_pin(&manifest.source.sha256)
        || !valid_hash_pin(&manifest.output.sha256)
    {
        return Err(import_error(
            "E_IMPORT_MANIFEST",
            "unsupported or inconsistent import manifest",
        ));
    }
    Ok(())
}

fn valid_hash_pin(pin: &str) -> bool {
    pin.len() == 71
        && pin.starts_with("sha256:")
        && pin[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn preflight_legacy_import_in_root(
    root: &bundle_fs::ProjectRoot,
) -> Result<(), MediaImportError> {
    let (_, manifest) = read_import_manifest(root)?;
    validate_import_manifest(&manifest)?;
    if manifest.version != MEDIA_IMPORT_VERSION || manifest.original.is_some() {
        return Err(import_error(
            "E_IMPORT_MANIFEST",
            "import.json is not a v1 import without original",
        ));
    }
    Ok(())
}

pub(crate) fn preflight_retained_import_in_root(
    root: &bundle_fs::ProjectRoot,
) -> Result<RetainedImportPreflight, MediaImportError> {
    let (json, manifest) = read_import_manifest(root)?;
    validate_import_manifest(&manifest)?;
    let original = manifest.original.as_ref().ok_or_else(|| {
        import_error(
            "E_IMPORT_MANIFEST",
            "retained import has no original identity",
        )
    })?;
    if manifest.version != MEDIA_IMPORT_RETAINED_VERSION
        || original.path != "original.wav"
        || original.bytes != manifest.source.bytes
        || original.sha256 != manifest.source.sha256
        || !valid_hash_pin(&original.sha256)
    {
        return Err(import_error(
            "E_IMPORT_MANIFEST",
            "unsupported or inconsistent retained import manifest",
        ));
    }
    Ok(RetainedImportPreflight { json, manifest })
}

pub(crate) fn verify_retained_import_snapshot_in_root(
    root: &bundle_fs::ProjectRoot,
    preflight: &RetainedImportPreflight,
) -> Result<ImportedWav, MediaImportError> {
    let manifest = &preflight.manifest;
    let original = bundle_fs::open_contained_member(root, "original.wav")
        .map_err(|error| import_error("E_REFERENCE", format!("original.wav: {error}")))?;
    let imported = import_wav_open_file(
        original,
        manifest.source.name.clone(),
        Some(WavImportRange::new(
            manifest.selection.start_frame,
            manifest.selection.end_frame,
        )),
        true,
    )?;
    if imported.input_bytes != manifest.source.bytes
        || imported.source_hash != manifest.source.sha256
        || imported.source_encoding != manifest.source.encoding
        || imported.source_rate_hz != manifest.source.rate_hz
        || imported.source_channels != manifest.source.channels
        || imported.source_frames != manifest.source.frames
        || imported.frames() != manifest.output.frames
        || imported.sample_rate_hz() != manifest.output.rate_hz
        || u16::from(imported.channels()) != manifest.output.channels
        || imported.pcm_hash != manifest.output.sha256
    {
        return Err(import_error(
            "E_IMPORT_MISMATCH",
            "retained WAV or decoded crop differs from import.json",
        ));
    }
    Ok(imported)
}

pub(crate) fn verify_retained_import_snapshot_disk_in_root(
    root: &bundle_fs::ProjectRoot,
    preflight: &RetainedImportPreflight,
) -> Result<DiskImportedWav, MediaImportError> {
    let manifest = &preflight.manifest;
    let original = bundle_fs::open_contained_member(root, "original.wav")
        .map_err(|error| import_error("E_REFERENCE", format!("original.wav: {error}")))?;
    let imported = import_wav_open_file_disk(
        original,
        manifest.source.name.clone(),
        Some(WavImportRange::new(
            manifest.selection.start_frame,
            manifest.selection.end_frame,
        )),
        true,
    )?;
    if imported.input_bytes != manifest.source.bytes
        || imported.source_hash != manifest.source.sha256
        || imported.source_encoding != manifest.source.encoding
        || imported.source_rate_hz != manifest.source.rate_hz
        || imported.source_channels != manifest.source.channels
        || imported.source_frames != manifest.source.frames
        || imported.frames() != manifest.output.frames
        || imported.source_rate_hz != manifest.output.rate_hz
        || imported.source_channels != manifest.output.channels
        || imported.pcm_hash != manifest.output.sha256
    {
        return Err(import_error(
            "E_IMPORT_MISMATCH",
            "retained WAV or decoded crop differs from import.json",
        ));
    }
    Ok(imported)
}

/// Verify the retained original and media.pcm with bounded buffers while
/// keeping the decoded crop in private file-backed storage.
pub fn verify_retained_import_disk(project: &Path) -> Result<DiskImportedWav, MediaImportError> {
    let root = bundle_fs::ProjectRoot::open_pinned(project, cap_std::ambient_authority())
        .map_err(|error| import_error("E_REFERENCE", format!("project root: {error}")))?;
    verify_retained_import_disk_in_root(&root)
}

pub(crate) fn verify_retained_import_disk_in_root(
    root: &bundle_fs::ProjectRoot,
) -> Result<DiskImportedWav, MediaImportError> {
    let preflight = preflight_retained_import_in_root(root)?;
    let imported = verify_retained_import_snapshot_disk_in_root(root, &preflight)?;
    let mut actual = bundle_fs::open_contained_member(root, "media.pcm")
        .map_err(|error| import_error("E_REFERENCE", format!("media.pcm: {error}")))?;
    let mut expected = imported
        .pcm_snapshot
        .reopen()
        .map_err(|error| import_error("E_IO", format!("cannot reopen PCM snapshot: {error}")))?;
    let mut actual_block = [0u8; SNAPSHOT_BUFFER_BYTES];
    let mut expected_block = [0u8; SNAPSHOT_BUFFER_BYTES];
    let mut remaining = imported.pcm_bytes_len;
    while remaining > 0 {
        let wanted = remaining.min(SNAPSHOT_BUFFER_BYTES as u64) as usize;
        expected
            .read_exact(&mut expected_block[..wanted])
            .map_err(|error| import_error("E_IO", format!("cannot read PCM snapshot: {error}")))?;
        actual
            .read_exact(&mut actual_block[..wanted])
            .map_err(|error| {
                import_error(
                    "E_IMPORT_MISMATCH",
                    format!("media.pcm ended early: {error}"),
                )
            })?;
        if actual_block[..wanted] != expected_block[..wanted] {
            return Err(import_error(
                "E_IMPORT_MISMATCH",
                "media.pcm differs from the decoded retained WAV crop",
            ));
        }
        remaining -= wanted as u64;
    }
    if actual
        .read(&mut actual_block[..1])
        .map_err(|error| import_error("E_IO", format!("cannot read media.pcm: {error}")))?
        != 0
    {
        return Err(import_error(
            "E_IMPORT_MISMATCH",
            "media.pcm differs from the decoded retained WAV crop",
        ));
    }
    Ok(imported)
}

/// Verify a retained import's original, declared crop, and exact native PCM.
/// Source closure compilation is performed separately by the CLI so edits to
/// the MaaC source do not invalidate this immutable import record.
pub fn verify_retained_import(project: &Path) -> Result<ImportedWav, MediaImportError> {
    let root = bundle_fs::ProjectRoot::open_pinned(project, cap_std::ambient_authority())
        .map_err(|error| import_error("E_REFERENCE", format!("project root: {error}")))?;
    verify_retained_import_in_root(&root)
}

pub(crate) fn verify_retained_import_in_root(
    root: &bundle_fs::ProjectRoot,
) -> Result<ImportedWav, MediaImportError> {
    let preflight = preflight_retained_import_in_root(root)?;
    let imported = verify_retained_import_snapshot_in_root(root, &preflight)?;
    let pcm = read_member(root, "media.pcm", MAX_BUNDLE_FILE_BYTES as u64)?;
    if pcm != imported.pcm_bytes {
        return Err(import_error(
            "E_IMPORT_MISMATCH",
            "media.pcm differs from the decoded retained WAV crop",
        ));
    }
    Ok(imported)
}

fn read_member(
    root: &bundle_fs::ProjectRoot,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, MediaImportError> {
    let file = bundle_fs::open_contained_member(root, name)
        .map_err(|error| import_error("E_REFERENCE", format!("{name}: {error}")))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| import_error("E_IO", format!("cannot read {name}: {error}")))?;
    if bytes.len() as u64 > limit {
        return Err(import_error(
            "E_RESOURCE_LIMIT",
            format!("{name} exceeds {limit} bytes"),
        ));
    }
    Ok(bytes)
}

fn import_error(code: &'static str, message: impl Into<String>) -> MediaImportError {
    MediaImportError {
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod retained_archive_tests {
    use super::*;
    use std::fs;

    #[test]
    fn private_original_stages_exactly_once_and_preflight_is_strict() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input.wav");
        let mut writer = hound::WavWriter::create(
            &input,
            WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [0i16, 1000, -2000] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let imported =
            import_wav_file_with_retention(&input, Some(WavImportRange::new(1, 3)), true).unwrap();
        let original = temp.path().join("original.wav");
        imported.copy_original_to(&original).unwrap();
        assert_eq!(fs::read(&input).unwrap(), fs::read(&original).unwrap());

        let manifest = temp.path().join("import.json");
        fs::write(&manifest, imported.manifest_json().unwrap()).unwrap();
        let root =
            bundle_fs::ProjectRoot::open_pinned(temp.path(), cap_std::ambient_authority()).unwrap();
        let preflight = preflight_retained_import_in_root(&root).unwrap();
        assert_eq!(
            preflight.original_bytes(),
            fs::metadata(&original).unwrap().len()
        );
        assert_eq!(
            verify_retained_import_snapshot_in_root(&root, &preflight)
                .unwrap()
                .pcm_bytes(),
            imported.pcm_bytes()
        );
        let mut value: serde_json::Value = serde_json::from_slice(preflight.json()).unwrap();
        value["extra"] = serde_json::json!(true);
        fs::write(manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(preflight_retained_import_in_root(&root).is_err());
    }

    #[test]
    fn disk_import_matches_memory_and_verifies_retained_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input.wav");
        let mut writer = hound::WavWriter::create(
            &input,
            WavSpec {
                channels: 2,
                sample_rate: 44_100,
                bits_per_sample: 24,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [0i32, 0, 1_234_567, -2_345_678, 8_388_607, -8_388_608] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let range = Some(WavImportRange::new(1, 3));
        let memory = import_wav_file_with_retention(&input, range, true).unwrap();
        let disk = import_wav_file_disk_with_retention(&input, range, true).unwrap();
        assert_eq!(
            disk.source_text(),
            memory.source_bundle().sources["main.maac"]
        );
        assert_eq!(
            disk.manifest_json().unwrap(),
            memory.manifest_json().unwrap()
        );
        assert_eq!(disk.pcm_hash(), memory.pcm_hash());
        assert_eq!(disk.pcm_bytes_len(), memory.pcm_bytes().len() as u64);

        fs::write(temp.path().join("main.maac"), disk.source_text()).unwrap();
        disk.stage_pcm_to(&temp.path().join("media.pcm")).unwrap();
        disk.stage_retained_original_to(&temp.path().join("original.wav"))
            .unwrap();
        fs::write(
            temp.path().join("import.json"),
            disk.manifest_json().unwrap(),
        )
        .unwrap();
        assert_eq!(
            fs::read(temp.path().join("media.pcm")).unwrap(),
            memory.pcm_bytes()
        );
        verify_retained_import_disk(temp.path()).unwrap();
        fs::write(temp.path().join("media.pcm"), [0u8; 16]).unwrap();
        assert_eq!(
            verify_retained_import_disk(temp.path()).unwrap_err().code(),
            "E_IMPORT_MISMATCH"
        );
    }

    #[test]
    fn disk_import_accepts_crop_above_in_memory_asset_limit() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("long.wav");
        let mut writer = hound::WavWriter::create(
            &input,
            WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        let frames = MAX_BUNDLE_FILE_BYTES as u64 / 4 + 1;
        for _ in 0..frames {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        assert_eq!(
            import_wav_file(&input, None).unwrap_err().code(),
            "E_RESOURCE_LIMIT"
        );
        let disk = import_wav_file_disk_with_retention(&input, None, false).unwrap();
        assert_eq!(disk.pcm_bytes_len(), frames * 4);
        assert_eq!(disk.manifest_version(), MEDIA_IMPORT_VERSION);
        let pcm = temp.path().join("media.pcm");
        disk.stage_pcm_to(&pcm).unwrap();
        assert_eq!(fs::metadata(pcm).unwrap().len(), frames * 4);
    }
}
