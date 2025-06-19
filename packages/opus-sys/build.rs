#[allow(deprecated)]
use bindgen::CargoCallbacks;
use {
    bindgen::builder,
    std::env::var,
};

fn main() {
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

    builder()
        .header(format!(
            "../../vcpkg_installed/{triplet}/include/opus/opus.h"
        ))
        .parse_callbacks(Box::new(CargoCallbacks::new()))
        .generate()
        .expect("failed to generate bindgen")
        .write_to_file(format!("{}/lib.rs", var("OUT_DIR").unwrap()))
        .expect("failed to write to file");

    println!("cargo:rustc-link-search=native=vcpkg_installed/{triplet}/lib");
    println!("cargo:rustc-link-lib=static=opus");
}
