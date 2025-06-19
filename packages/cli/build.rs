use {
    ::bindgen::builder,
    ::std::{
        env::var,
        process::Command,
    },
};

fn main() {
    println!("cargo:rerun-if-changed=vcpkg.json");

    let arch = match var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        arch => panic!("unsupported arch: {arch}"),
    };

    let os = match var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
        "windows" => "windows-static-md",
        "macos" => "osx",
        "linux" => "linux",
        os => panic!("unsupported os: {os}"),
    };

    let triplet = format!("{arch}-{os}");

    let status = Command::new("vcpkg")
        .args(["install", "--triplet", &triplet])
        .output()
        .expect("failed to spawn vcpkg command")
        .status;

    assert!(
        status.success(),
        "vcpkg command failed: {:?}",
        status.code()
    );

    builder()
        .raw_line("#![allow(dead_code, non_camel_case_types)]")
        .header(format!(
            "../../vcpkg_installed/{triplet}/include/opus/opus.h"
        ))
        .generate()
        .expect("failed to generate bindgen")
        .write_to_file("src/opus.rs")
        .expect("failed to write to file");

    builder()
        .raw_line("#![allow(dead_code, non_camel_case_types)]")
        .header(format!(
            "../../vcpkg_installed/{triplet}/include/speex/speex_resampler.h"
        ))
        .generate()
        .expect("failed to generate bindgen")
        .write_to_file("src/speex.rs")
        .expect("failed to write to file");

    println!("cargo:rustc-link-search=native=vcpkg_installed/{triplet}/lib");
    println!("cargo:rustc-link-lib=static=opus");
    println!("cargo:rustc-link-lib=static=speexdsp");
}
