use {
    ::bindgen::builder,
    ::std::{
        env::var,
        process::Command,
    },
};

fn main() {
    println!("cargo:rerun-if-changed=vcpkg.json");

    let status = Command::new("vcpkg")
        .args(["install"])
        .output()
        .expect("failed to spawn vcpkg command")
        .status;

    assert!(
        status.success(),
        "vcpkg command failed: {:?}",
        status.code()
    );

    let arch = match var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        arch => panic!("unsupported arch: {arch}"),
    };

    let os = match var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
        "windows" => "windows",
        "macos" => "osx",
        "linux" => "linux",
        os => panic!("unsupported os: {os}"),
    };

    builder()
        .raw_line("#![allow(unused)]")
        .header(format!("vcpkg_installed/{arch}-{os}/include/opus/opus.h"))
        .generate()
        .expect("failed to generate bindgen")
        .write_to_file("src/opus.rs")
        .expect("failed to write to file");

    println!("cargo:rustc-link-search=native=vcpkg_installed/{arch}-{os}/lib");
    println!("cargo:rustc-link-lib=static=opus");
}
