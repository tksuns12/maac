use maac::wavetable::{TableBank, Wavetable, TABLE_BANK_VERSION};

#[test]
fn embeds_explicit_mono_cycles_without_normalizing_them() {
    let samples = [-32768_i16, -16384, 0, 8192, 16384, 24576, 32767, 0];
    let bytes = pcm16_wav(&samples, 22_050, 1);

    let table = Wavetable::from_wav("fixture", &bytes, 8).unwrap();

    assert_eq!(table.id, "fixture");
    assert_eq!(table.cycle_length, 8);
    assert_eq!(table.samples.len(), 8);
    assert_eq!(table.samples[0], -1.0);
    assert_eq!(table.samples[1], -0.5);
    assert_eq!(table.samples[3], 0.25);
    assert_eq!(table.samples[6], 32767.0 / 32768.0);
    table.validate().unwrap();

    let float_bytes = float32_wav(&[0.0, -0.5, 0.25, 1.5, -2.0, 0.0, 0.0, 0.0]);
    let float_table = Wavetable::from_wav("float", &float_bytes, 8).unwrap();
    assert_eq!(float_table.samples[1], -0.5);
    assert_eq!(float_table.samples[3], 1.5);
    assert_eq!(float_table.samples[4], -2.0);
}

#[test]
fn table_bank_exposes_a_versioned_reusable_contract() {
    let table = Wavetable {
        id: "constant".into(),
        cycle_length: 8,
        samples: vec![0.25; 8],
    };

    let bank = TableBank::new(&table).unwrap();

    assert_eq!(TABLE_BANK_VERSION, 1);
    assert_eq!(bank.version(), TABLE_BANK_VERSION);
    assert_eq!(bank.sample(0.0, 0.0, 0.0, 48_000.0).unwrap(), 0.25);
}

#[test]
fn cyclic_interpolation_wraps_negative_phase_and_morphs_adjacent_frames() {
    let table = Wavetable {
        id: "morph".into(),
        cycle_length: 8,
        samples: vec![
            0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, // frame 0
            100.0, 102.0, 104.0, 106.0, 108.0, 110.0, 112.0, 114.0, // frame 1
        ],
    };
    let bank = TableBank::new(&table).unwrap();

    // -1/16 cycle lies halfway from the last sample to the first. Position
    // 0.25 blends one quarter of the way from frame zero to frame one.
    assert_eq!(bank.sample(-1.0 / 16.0, 0.25, 0.0, 48_000.0).unwrap(), 32.0);
    assert_eq!(bank.sample(1.0, 1.0, 0.0, 48_000.0).unwrap(), 100.0);
    assert_eq!(
        bank.sample(-f64::MIN_POSITIVE, 0.0, 0.0, 48_000.0).unwrap(),
        0.0
    );
}

#[test]
fn rejects_invalid_or_untrusted_wav_data_and_embedded_samples() {
    let stereo = pcm16_wav(&[0; 16], 48_000, 2);
    assert_eq!(
        Wavetable::from_wav("stereo", &stereo, 8).unwrap_err().code,
        "E_RANGE"
    );

    let partial_cycle = pcm16_wav(&[0; 9], 48_000, 1);
    assert_eq!(
        Wavetable::from_wav("partial", &partial_cycle, 8)
            .unwrap_err()
            .code,
        "E_RANGE"
    );

    let mut truncated = pcm16_wav(&[0; 8], 48_000, 1);
    truncated.pop();
    assert_eq!(
        Wavetable::from_wav("truncated", &truncated, 8)
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );
    assert_eq!(
        Wavetable::from_wav("malformed", b"not a wave file", 8)
            .unwrap_err()
            .code,
        "E_SYNTAX"
    );
    assert_eq!(
        Wavetable::from_wav("bad-cycle", &pcm16_wav(&[0; 8], 48_000, 1), 12)
            .unwrap_err()
            .code,
        "E_RANGE"
    );

    let oversized = vec![0; maac::wavetable::MAX_WAVETABLE_BYTES + 1];
    assert_eq!(
        Wavetable::from_wav("oversized", &oversized, 8)
            .unwrap_err()
            .code,
        "E_RESOURCE_LIMIT"
    );

    let nonfinite = Wavetable {
        id: "nonfinite".into(),
        cycle_length: 8,
        samples: vec![f64::NAN; 8],
    };
    assert_eq!(nonfinite.validate().unwrap_err().code, "E_NONFINITE");
    assert_eq!(TableBank::new(&nonfinite).unwrap_err().code, "E_NONFINITE");

    let mut unsupported = pcm16_wav(&[0; 8], 48_000, 1);
    // Eight valid bits in a two-byte PCM container passes structural header
    // checks but is not a representation supported by hound's decoder.
    unsupported[34..36].copy_from_slice(&8_u16.to_le_bytes());
    assert_eq!(
        Wavetable::from_wav("unsupported", &unsupported, 8)
            .unwrap_err()
            .code,
        "E_CAPABILITY"
    );
}

#[test]
fn rejects_nonfinite_float_wav_and_unknown_serialized_fields() {
    let bytes = float32_wav(&[0.0, 0.25, f32::INFINITY, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(
        Wavetable::from_wav("float", &bytes, 8).unwrap_err().code,
        "E_NONFINITE"
    );

    let wire = r#"{"id":"strict","cycle_length":8,"samples":[0,0,0,0,0,0,0,0],"extra":true}"#;
    assert!(serde_json::from_str::<Wavetable>(wire).is_err());
}

#[test]
fn harmonic_bank_removes_illegal_partial_but_retains_dc_and_legal_harmonic() {
    let cycle_length = 32_u32;
    let raw: Vec<f64> = (0..cycle_length)
        .map(|index| {
            let phase = index as f64 / cycle_length as f64;
            0.25 + 0.5 * (std::f64::consts::TAU * phase).sin()
                + 0.75 * (std::f64::consts::TAU * 8.0 * phase).sin()
        })
        .collect();
    let table = Wavetable {
        id: "spectrum".into(),
        cycle_length,
        samples: raw.clone(),
    };
    let first_bank = TableBank::new(&table).unwrap();
    let second_bank = TableBank::new(&table).unwrap();
    assert_eq!(first_bank, second_bank);

    let phase = 1.0 / cycle_length as f64;
    let rich = first_bank.sample(phase, 0.0, 0.0, 48_000.0).unwrap();
    assert!((rich - raw[1]).abs() < 1e-15);

    // At 4 kHz the selected ceiling is four: harmonic eight is illegal, while
    // DC and the fundamental remain. Negative frequency selects the same bank.
    let expected = 0.25 + 0.5 * (std::f64::consts::TAU * phase).sin();
    let positive = first_bank.sample(phase, 0.0, 4_000.0, 48_000.0).unwrap();
    let negative = first_bank.sample(phase, 0.0, -4_000.0, 48_000.0).unwrap();
    assert!(
        (positive - expected).abs() < 1e-12,
        "{positive} != {expected}"
    );
    assert_eq!(positive, negative);
    assert_eq!(
        positive,
        first_bank.sample(phase, 0.0, 4_000.0, 48_000.0).unwrap()
    );
}

#[test]
fn nyquist_bin_is_retained_only_when_legal_for_the_selected_bank() {
    let table = Wavetable {
        id: "nyquist-bin".into(),
        cycle_length: 8,
        samples: (0..8)
            .map(|index| if index % 2 == 0 { 1.0 } else { -1.0 })
            .collect(),
    };
    let bank = TableBank::new(&table).unwrap();

    assert_eq!(bank.sample(0.0, 0.0, 0.0, 48_000.0).unwrap(), 1.0);
    assert!(bank.sample(0.0, 0.0, 10_000.0, 48_000.0).unwrap().abs() < 1e-12);
}

#[test]
fn sample_rejects_out_of_contract_runtime_parameters() {
    let bank = TableBank::new(&Wavetable {
        id: "parameters".into(),
        cycle_length: 8,
        samples: vec![0.0; 8],
    })
    .unwrap();

    assert!(bank.sample(f64::NAN, 0.0, 0.0, 48_000.0).is_err());
    assert!(bank.sample(0.0, 1.01, 0.0, 48_000.0).is_err());
    assert!(bank.sample(0.0, 0.0, 24_000.1, 48_000.0).is_err());
    assert!(bank.sample(0.0, 0.0, 0.0, 44_100.0).is_err());
}

#[test]
fn preprocessing_accepts_the_published_per_table_maximum() {
    let table = Wavetable {
        id: "maximum".into(),
        cycle_length: 2_048,
        samples: vec![0.125; maac::wavetable::MAX_WAVETABLE_SAMPLES],
    };

    let bank = TableBank::new(&table).unwrap();
    assert!((bank.sample(0.7, 0.63, 12_000.0, 48_000.0).unwrap() - 0.125).abs() < 1e-12);
}

fn pcm16_wav(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
        for &sample in samples {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }
    cursor.into_inner()
}

fn float32_wav(samples: &[f32]) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 96_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
        for &sample in samples {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }
    cursor.into_inner()
}
