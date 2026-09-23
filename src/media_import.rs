//! Bounded WAV import into MaaC's exact interleaved float32 audio-asset format.
//!
//! The input is snapshotted to temporary storage and hashed before decoding so
//! the recorded source identity is the identity of the bytes that were read.
//! Only the selected crop is retained in memory as a MaaC asset.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use hound::{SampleFormat, WavSpec};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::bundle::{sha256_digest, SourceBundle, MAX_BUNDLE_FILE_BYTES};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub const MEDIA_IMPORT_FORMAT: &str = "maac.media-import";
pub const MEDIA_IMPORT_VERSION: u32 = 1;
pub const MEDIA_IMPORT_DECODER_ID: &str = "maac.wav-import/1";
pub const MEDIA_IMPORT_CONVERSION_ID: &str = "maac.pcm-f32le-conversion/1";
pub const MAX_MEDIA_IMPORT_INPUT_BYTES: u64 = 1024 * 1024 * 1024;
const SNAPSHOT_BUFFER_BYTES: usize = 64 * 1024;
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
}

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

    /// Build the ordinary source bundle that is written by the CLI and can
    /// subsequently pass through the existing check/build/compile pipeline.
    pub fn source_bundle(&self) -> SourceBundle {
        let source = format!(
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
            self.frames(),
            self.source_rate_hz,
            self.pcm_hash,
            CORE_AUDIO_FORMAT,
            self.source_rate_hz,
            self.source_channels,
            self.frames(),
            self.frames(),
        );
        SourceBundle {
            entry: "main.maac".into(),
            sources: [("main.maac".into(), source)].into(),
            assets: [("media.pcm".into(), self.pcm_bytes.clone())].into(),
        }
    }

    pub fn manifest_json(&self) -> Result<Vec<u8>, MediaImportError> {
        let manifest = MediaImportManifest {
            format: MEDIA_IMPORT_FORMAT,
            version: MEDIA_IMPORT_VERSION,
            decoder: MEDIA_IMPORT_DECODER_ID,
            conversion: MEDIA_IMPORT_CONVERSION_ID,
            source: MediaImportSource {
                name: &self.input_name,
                bytes: self.input_bytes,
                sha256: &self.source_hash,
                container: "riff_wave",
                encoding: &self.source_encoding,
                rate_hz: self.source_rate_hz,
                channels: self.source_channels,
                frames: self.source_frames,
            },
            selection: MediaImportSelection {
                start_frame: self.range.start_frame,
                end_frame: self.range.end_frame,
            },
            output: MediaImportOutput {
                path: "media.pcm",
                format: CORE_AUDIO_FORMAT,
                rate_hz: self.source_rate_hz,
                channels: self.source_channels,
                frames: self.frames(),
                sha256: &self.pcm_hash,
            },
        };
        serde_json::to_vec_pretty(&manifest).map_err(|error| {
            import_error(
                "E_INTERNAL",
                format!("cannot encode import manifest: {error}"),
            )
        })
    }
}

/// Snapshot, identify, and convert one WAV file into the supported native PCM
/// asset. With no `range`, the complete nonempty WAV is imported.
pub fn import_wav_file(
    input_path: &Path,
    range: Option<WavImportRange>,
) -> Result<ImportedWav, MediaImportError> {
    let (snapshot, input_bytes, source_hash) = snapshot_and_hash(input_path)?;
    let input_name = input_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "input.wav".into());

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
        let converted = match (spec.sample_format, sample_bytes) {
            (SampleFormat::Int, 2) => {
                f64::from(i16::from_le_bytes([bytes[0], bytes[1]])) as f32 / 32_768.0
            }
            (SampleFormat::Int, 3) => {
                let sign = if bytes[2] & 0x80 == 0 { 0 } else { 0xff };
                let sample = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], sign]);
                (f64::from(sample) / 8_388_608.0) as f32
            }
            (SampleFormat::Int, 4) => {
                (f64::from(i32::from_le_bytes(bytes)) / 2_147_483_648.0) as f32
            }
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
        };
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
    })
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

fn snapshot_and_hash(input_path: &Path) -> Result<(NamedTempFile, u64, String), MediaImportError> {
    let mut input = open_import_input(input_path).map_err(|error| {
        import_error(
            "E_IO",
            format!("cannot open {}: {error}", input_path.display()),
        )
    })?;
    let metadata = input.metadata().map_err(|error| {
        import_error(
            "E_IO",
            format!("cannot inspect {}: {error}", input_path.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err(import_error(
            "E_REFERENCE",
            format!("input is not a regular file: {}", input_path.display()),
        ));
    }
    if metadata.len() > MAX_MEDIA_IMPORT_INPUT_BYTES {
        return Err(import_error(
            "E_RESOURCE_LIMIT",
            format!(
                "WAV input exceeds {MAX_MEDIA_IMPORT_INPUT_BYTES} bytes: {}",
                input_path.display()
            ),
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

fn import_error(code: &'static str, message: impl Into<String>) -> MediaImportError {
    MediaImportError {
        code,
        message: message.into(),
    }
}
