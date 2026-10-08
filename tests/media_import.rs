use std::{
    fs,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use hound::{SampleFormat, WavSpec, WavWriter};
use maac::{
    compile_bundle_artifact,
    media_import::{import_wav_file, WavImportRange},
};
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn pcm16_crop_imports_as_compilable_native_audio_with_provenance() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("recording.wav");
    let spec = WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&input, spec).unwrap();
    for sample in [i16::MIN, i16::MAX, 16_384, -16_384, 0, 1, -1, 8_192] {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();

    let input_bytes = fs::read(&input).unwrap();
    let imported = import_wav_file(&input, Some(WavImportRange::new(1, 3))).unwrap();

    assert_eq!(
        imported.source_hash(),
        maac::bundle::sha256_digest(&input_bytes)
    );
    assert_eq!(imported.source_frames(), 4);
    assert_eq!(imported.range(), WavImportRange::new(1, 3));
    assert_eq!(imported.channels(), 2);
    assert_eq!(imported.sample_rate_hz(), 48_000);

    let pcm = imported.pcm_bytes();
    let values = pcm
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect::<Vec<_>>();
    assert_eq!(values, [0.5, -0.5, 0.0, 1.0 / 32768.0]);

    let manifest: Value = serde_json::from_slice(&imported.manifest_json().unwrap()).unwrap();
    assert_eq!(manifest["format"], "maac.media-import");
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["source"]["encoding"], "pcm_s16le");
    assert_eq!(manifest["source"]["frames"], 4);
    assert_eq!(manifest["selection"]["start_frame"], 1);
    assert_eq!(manifest["selection"]["end_frame"], 3);
    assert_eq!(manifest["output"]["frames"], 2);
    assert_eq!(
        manifest["output"]["sha256"],
        maac::bundle::sha256_digest(pcm)
    );
    assert!(imported
        .source_bundle()
        .sources
        .keys()
        .any(|path| path == "main.maac"));

    let plan = compile_bundle_artifact(&imported.source_bundle()).unwrap();
    assert_eq!(plan.output().total_frames, 2);
    assert_eq!(plan.audio_clip_count(), 1);
    assert!(plan.to_json().is_ok());
}

#[test]
fn float32_import_preserves_signed_zero_and_subnormal_bits() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("float.wav");
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let bits = [0u32, 0x8000_0000, 1, 0x3f00_0000];
    let mut writer = WavWriter::create(&input, spec).unwrap();
    for value in bits.map(f32::from_bits) {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();

    let imported = import_wav_file(&input, Some(WavImportRange::new(0, 3))).unwrap();
    assert_eq!(
        imported
            .pcm_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| u32::from_le_bytes(*bytes))
            .collect::<Vec<_>>(),
        bits[..3]
    );
}

#[test]
fn pcm24_import_uses_signed_full_scale_without_gain_adjustment() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("pcm24.wav");
    let spec = WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 24,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&input, spec).unwrap();
    for value in [0x7f_ffffi32, -0x80_0000, 0x40_0000, -0x40_0000] {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();

    let imported = import_wav_file(&input, None).unwrap();
    let values = imported
        .pcm_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect::<Vec<_>>();
    assert_eq!(imported.source_encoding(), "pcm_s24le");
    assert_eq!(values, [8_388_607.0 / 8_388_608.0, -1.0, 0.5, -0.5]);
    let plan = compile_bundle_artifact(&imported.source_bundle()).unwrap();
    assert_eq!(plan.audio_clip_count(), 1);
    assert!(plan.output().total_frames > 0);
}

#[test]
fn padded_pcm24_stereo_is_rejected_before_crop_seek_can_misalign_samples() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("padded-pcm24.wav");
    let mut data = Vec::new();
    for sample in [0x7f_ffffi32, -0x80_0000, 0x40_0000, -0x40_0000] {
        data.extend_from_slice(&sample.to_le_bytes());
    }
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&48_000u32.to_le_bytes());
    wav.extend_from_slice(&(48_000u32 * 8).to_le_bytes());
    wav.extend_from_slice(&8u16.to_le_bytes());
    wav.extend_from_slice(&24u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    fs::write(&input, wav).unwrap();

    let error = import_wav_file(&input, Some(WavImportRange::new(1, 2))).unwrap_err();
    assert_eq!(error.code(), "E_CAPABILITY");
    assert!(error.message().contains("padded WAV sample containers"));
}

fn riff_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::new();
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    chunk.extend_from_slice(payload);
    if payload.len() & 1 != 0 {
        chunk.push(0);
    }
    chunk
}

fn riff_wave(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut wave = b"RIFF".to_vec();
    let size = 4 + chunks.iter().map(Vec::len).sum::<usize>();
    wave.extend_from_slice(&(size as u32).to_le_bytes());
    wave.extend_from_slice(b"WAVE");
    for chunk in chunks {
        wave.extend_from_slice(chunk);
    }
    wave
}

fn pcm_format(channels: u16, bits: u16, block_align: u16) -> Vec<u8> {
    let mut format = Vec::new();
    format.extend_from_slice(&1u16.to_le_bytes());
    format.extend_from_slice(&channels.to_le_bytes());
    format.extend_from_slice(&48_000u32.to_le_bytes());
    format.extend_from_slice(&(48_000u32 * u32::from(block_align)).to_le_bytes());
    format.extend_from_slice(&block_align.to_le_bytes());
    format.extend_from_slice(&bits.to_le_bytes());
    format
}

#[test]
fn odd_sized_chunks_before_format_and_data_are_skipped_with_their_pad_bytes() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("odd-chunks.wav");
    let wave = riff_wave(&[
        riff_chunk(b"JUNK", &[0x11]),
        riff_chunk(b"fmt ", &pcm_format(1, 16, 2)),
        riff_chunk(b"LIST", &[0x22, 0x33, 0x44]),
        riff_chunk(b"data", &[0, 64, 0, 192]),
    ]);
    fs::write(&input, wave).unwrap();

    let imported = import_wav_file(&input, Some(WavImportRange::new(1, 2))).unwrap();
    assert_eq!(imported.source_frames(), 2);
    assert_eq!(imported.source_encoding(), "pcm_s16le");
    assert_eq!(imported.pcm_bytes(), &(-0.5f32).to_le_bytes());
}

#[test]
fn terminal_odd_sized_pcm24_data_without_a_pad_remains_decodable() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("terminal-odd-data.wav");
    let mut wave = riff_wave(&[
        riff_chunk(b"fmt ", &pcm_format(1, 24, 3)),
        riff_chunk(b"data", &[0, 0, 64]),
    ]);
    wave.pop(); // Hound-style final data chunk without an optional pad byte.
    let riff_size = (wave.len() - 8) as u32;
    wave[4..8].copy_from_slice(&riff_size.to_le_bytes());
    fs::write(&input, wave).unwrap();

    let imported = import_wav_file(&input, None).unwrap();
    assert_eq!(imported.source_frames(), 1);
    assert_eq!(imported.pcm_bytes(), &0.5f32.to_le_bytes());
}

#[test]
fn duplicate_format_cannot_override_a_packed_format_with_a_padded_one() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("duplicate-format.wav");
    let wave = riff_wave(&[
        riff_chunk(b"fmt ", &pcm_format(1, 24, 3)),
        riff_chunk(b"fmt ", &pcm_format(1, 24, 4)),
        riff_chunk(b"data", &[0, 0, 64, 0]),
    ]);
    fs::write(&input, wave).unwrap();

    let error = import_wav_file(&input, None).unwrap_err();
    assert_eq!(error.code(), "E_WAV");
    assert!(error.message().contains("duplicate WAV format chunk"));
}

#[test]
fn duplicate_data_after_first_data_is_rejected() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("duplicate-data.wav");
    let wave = riff_wave(&[
        riff_chunk(b"fmt ", &pcm_format(1, 16, 2)),
        riff_chunk(b"data", &[0, 64]),
        riff_chunk(b"data", &[0, 192]),
    ]);
    fs::write(&input, wave).unwrap();

    let error = import_wav_file(&input, None).unwrap_err();
    assert_eq!(error.code(), "E_WAV");
    assert!(error.message().contains("duplicate WAV data chunk"));
}

#[cfg(unix)]
#[test]
fn fifo_import_returns_promptly_instead_of_waiting_for_a_writer() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let directory = tempdir().unwrap();
    let fifo = directory.path().join("input.wav");
    let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: fifo_name is a valid, NUL-terminated path and mode is a valid
    // permission mask.
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "fifo_import_helper", "--nocapture"])
        .env("MAAC_FIFO_IMPORT_HELPER", "1")
        .env("MAAC_FIFO_IMPORT_PATH", &fifo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{output:?}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("FIFO import subprocess did not exit promptly: {output:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn fifo_import_helper() {
    let Some(path) = std::env::var_os("MAAC_FIFO_IMPORT_PATH") else {
        return;
    };
    if std::env::var_os("MAAC_FIFO_IMPORT_HELPER").is_none() {
        return;
    }
    let error =
        import_wav_file(std::path::Path::new(&path), Some(WavImportRange::new(0, 1))).unwrap_err();
    assert_eq!(error.code(), "E_REFERENCE");
}

#[test]
fn imports_a_small_crop_from_wav_larger_than_the_native_asset_file_limit() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("long.wav");
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&input, spec).unwrap();
    let source_frames = 2_100_000u32;
    for _ in 0..source_frames {
        writer.write_sample(0i16).unwrap();
    }
    writer.finalize().unwrap();
    assert!(fs::metadata(&input).unwrap().len() > 4 * 1024 * 1024);

    let imported = import_wav_file(
        &input,
        Some(WavImportRange::new(
            u64::from(source_frames - 2),
            u64::from(source_frames),
        )),
    )
    .unwrap();
    assert_eq!(imported.source_frames(), source_frames);
    assert_eq!(imported.frames(), 2);
    assert_eq!(imported.pcm_bytes().len(), 8);
    assert_eq!(imported.input_bytes(), fs::metadata(&input).unwrap().len());
}

#[test]
fn invalid_ranges_and_selected_nonfinite_float_samples_fail_explicitly() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("float.wav");
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(&input, spec).unwrap();
    for value in [0.25f32, f32::NAN, 0.5] {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();

    assert_eq!(
        import_wav_file(&input, Some(WavImportRange::new(3, 3)))
            .unwrap_err()
            .code(),
        "E_RANGE"
    );
    assert_eq!(
        import_wav_file(&input, Some(WavImportRange::new(0, 4)))
            .unwrap_err()
            .code(),
        "E_RANGE"
    );
    assert_eq!(
        import_wav_file(&input, Some(WavImportRange::new(1, 2)))
            .unwrap_err()
            .code(),
        "E_ASSET"
    );
    assert!(import_wav_file(&input, Some(WavImportRange::new(0, 1))).is_ok());
}
