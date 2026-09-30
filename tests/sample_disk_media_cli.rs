//! Asset-backed `sample` declarations use `--disk-media` like audio assets.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

/// Beyond the 16 MiB inline asset limit.
const LONG_FRAMES: u64 = 5_000_001;
const LEAD: [f32; 8] = [0.5, -0.5, 0.25, 0.75, -0.125, 0.375, -0.25, 0.625];

fn invoke(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maac"))
        .args(args)
        .output()
        .unwrap()
}

fn result(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn hash_file(path: &Path) -> String {
    let mut file = File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    format!("sha256:{:x}", hash.finalize())
}

/// A project whose sampler plays `frames` of native PCM beginning with LEAD.
fn project(root: &Path, frames: u64) {
    fs::create_dir(root).unwrap();
    let pcm = root.join("keys.pcm");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pcm)
        .unwrap();
    for value in LEAD {
        file.write_all(&value.to_le_bytes()).unwrap();
    }
    file.set_len(frames * 4).unwrap();
    drop(file);
    let source = format!(
        "maac 1;\n\
         project p {{ score=[0q,1/3000q]; tail=0s; rate=48000Hz; tempo=&clock; meter=&metre; output=&keys:out; }}\n\
         tempo clock {{ points=[(0q,120bpm,step)]; }}\n\
         meter metre {{ points=[(0q,4,4)]; }}\n\
         sample tone {{ path=\"keys.pcm\"; hash=\"{}\"; format=\"pcm_f32le_interleaved/1\"; rate=48000Hz; frames={frames}; root=A4; }}\n\
         instrument sampler {{ channels=1; voice v {{ channels=1; amplitude=&amp; output=&play:out; node amp {{ type=\"synth.adsr/1\"; }} node play {{ type=\"synth.sample/1\"; config={{ zones=[{{ sample=&tone; low=key(0); high=key(127); }}]; }}; }} }} }}\n\
         node keys {{ instrument=&sampler; }}\n\
         pattern phrase {{ length=1/3000q; note n {{ at=0q; dur=1/3000q; pitch=A4; }} }}\n\
         track melody {{ target=&keys:events; }}\n\
         place play {{ pattern=&phrase; track=&melody; at=0q; }}\n",
        hash_file(&pcm)
    );
    fs::write(root.join("main.maac"), source).unwrap();
}

#[test]
fn oversized_sample_builds_only_as_disk_media_and_matches_inline_audio() {
    let directory = tempdir().unwrap();
    let long = directory.path().join("long");
    let short = directory.path().join("short");
    let disk_wav = directory.path().join("disk.wav");
    let inline_wav = directory.path().join("inline.wav");
    project(&long, LONG_FRAMES);
    project(&short, 1_000);

    let refused = invoke(&[Path::new("--json"), Path::new("check"), &long]);
    assert!(!refused.status.success(), "{refused:?}");
    let refused: Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(refused["code"], "E_RESOURCE_LIMIT");

    result(invoke(&[
        Path::new("--json"),
        Path::new("check"),
        &long,
        Path::new("--disk-media"),
    ]));
    result(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &long,
        Path::new("--disk-media"),
        Path::new("-o"),
        &disk_wav,
    ]));
    result(invoke(&[
        Path::new("--json"),
        Path::new("build"),
        &short,
        Path::new("-o"),
        &inline_wav,
    ]));
    // Both render the same leading recorded frames at the root rate.
    assert_eq!(fs::read(&disk_wav).unwrap(), fs::read(&inline_wav).unwrap());
    let mut reader = hound::WavReader::open(&disk_wav).unwrap();
    let rendered: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(&rendered[..LEAD.len()], &LEAD);
}
