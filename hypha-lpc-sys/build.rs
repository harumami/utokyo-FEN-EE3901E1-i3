#[allow(deprecated)]
use ::bindgen::CargoCallbacks;
use {
    ::bindgen::builder,
    ::cmake::Config,
    ::std::env::var,
};

fn main() {
    let dist = Config::new(".")
        .define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL")
        .build();

    let mut header = dist.clone();
    header.extend(["include", "hypha-lpc", "LPC.h"]);
    let header = header.into_os_string().into_string().unwrap();
    let mut lib = dist;
    lib.extend(["lib"]);
    let lib = lib.into_os_string().into_string().unwrap();

    builder()
        .header(header)
        .parse_callbacks(Box::new(CargoCallbacks::new()))
        .generate()
        .expect("failed to generate bindgen")
        .write_to_file(format!("{}/lib.rs", var("OUT_DIR").unwrap()))
        .expect("failed to write to file");

    println!("cargo:rustc-link-search=native={lib}");
    println!("cargo:rustc-link-lib=static=hypha-lpc");
}
