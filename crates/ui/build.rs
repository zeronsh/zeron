use std::{env, path::PathBuf, process::Command};

fn build_dictation() {
    println!("cargo:rerun-if-changed=src/dictation/macos.m");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = out.join("dictation.o");
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => panic!("Unsupported macOS architecture: {other}"),
    };
    let minimum = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "12.0".into());
    let status = Command::new("xcrun")
        .args([
            "clang",
            "-arch",
            arch,
            "-fobjc-arc",
            "-fblocks",
            "-Wall",
            "-Wextra",
            "-Wno-unused-parameter",
            "-O2",
        ])
        .arg(format!("-mmacosx-version-min={minimum}"))
        .args(["-c", "src/dictation/macos.m", "-o"])
        .arg(&object)
        .status()
        .expect("Xcode command line tools are required for macOS dictation");
    assert!(
        status.success(),
        "macOS dictation bridge compilation failed"
    );
    let status = Command::new("xcrun")
        .args(["ar", "crs"])
        .arg(out.join("libzeron_dictation.a"))
        .arg(object)
        .status()
        .unwrap();
    assert!(status.success(), "macOS dictation bridge archive failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=zeron_dictation");
    for framework in ["Speech", "AVFoundation", "AppKit"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_dictation();
    }
    println!("cargo:rerun-if-changed=src/browser/linux/helper.c");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("zeron-webkit");
    let flags = Command::new("pkg-config")
        .args(["--cflags", "--libs", "webkit2gtk-4.1", "json-glib-1.0"])
        .output()
        .expect("pkg-config is required to build the Linux browser helper");
    assert!(
        flags.status.success(),
        "The Linux browser requires WebKitGTK 4.1 and JSON-GLib development files \
         discoverable by pkg-config (webkit2gtk-4.1 and json-glib-1.0). \
         See docs/reference/linux-browser.md for distribution-specific installation commands.\n{}",
        String::from_utf8_lossy(&flags.stderr)
    );
    let status = Command::new(env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Wno-unused-parameter",
            "src/browser/linux/helper.c",
            "-o",
        ])
        .arg(&output)
        .args(String::from_utf8(flags.stdout).unwrap().split_whitespace())
        .status()
        .expect("C compiler is required to build the Linux browser helper");
    assert!(status.success(), "Linux browser helper compilation failed");
}
