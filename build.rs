use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=native/microphone_permission.m");
    println!("cargo:rerun-if-changed=native/Info.plist");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    let object = output.join("microphone_permission.o");
    let target = env::var("TARGET").expect("Cargo supplies TARGET");
    let status = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "clang",
            "-target",
            &target,
            "-fobjc-arc",
            "-fblocks",
            "-mmacosx-version-min=10.15",
            "-c",
            "native/microphone_permission.m",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("Xcode clang is required for macOS microphone permissions");
    assert!(
        status.success(),
        "cannot compile macOS microphone permission shim"
    );
    let status = Command::new("xcrun")
        .args(["ar", "crs"])
        .arg(output.join("libmaac_microphone.a"))
        .arg(&object)
        .status()
        .expect("Xcode ar is required");
    assert!(
        status.success(),
        "cannot archive macOS microphone permission shim"
    );
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=maac_microphone");
    for framework in [
        "AVFoundation",
        "Foundation",
        "AudioToolbox",
        "CoreAudio",
        "CoreFoundation",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    let plist =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies manifest root"))
            .join("native/Info.plist");
    println!(
        "cargo:rustc-link-arg-bin=maac=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}
