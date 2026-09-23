//! Builds the upstream libvmx C++ sources (from `reference/libvmx`, which is
//! gitignored and never shipped) so the `vmx-codec` tests can compare against
//! it. If the sources are missing, the crate builds as an empty stub and the
//! conformance tests skip themselves.

use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(libvmx_missing)");
    println!("cargo:rerun-if-env-changed=LIBVMX_SRC");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = std::env::var("LIBVMX_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("../../reference/libvmx/src"));
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-changed=csrc/shim.cpp");

    if !src.join("vmxcodec.cpp").exists() {
        println!(
            "cargo:warning=libvmx sources not found at {} - conformance tests will be skipped",
            src.display()
        );
        println!("cargo:rustc-cfg=libvmx_missing");
        return;
    }

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let mut b = cc::Build::new();
    b.cpp(true)
        .include(&src)
        .file(src.join("vmxcodec.cpp"))
        .file("csrc/shim.cpp")
        .opt_level(3)
        .debug(false)
        .warnings(false)
        .flag_if_supported("-std=c++17")
        .flag_if_supported("-fdeclspec")
        .flag_if_supported("-Wno-c++11-narrowing")
        .flag_if_supported("-Wno-narrowing")
        .flag_if_supported("-Wno-#warnings")
        .flag_if_supported("-Wno-cpp");
    let compiler = b.get_compiler();
    if compiler.is_like_msvc() {
        // As upstream's VMXCodec.vcxproj: C++17, standard preprocessor.
        b.flag("/std:c++17").flag("/Zc:preprocessor");
    } else if !compiler.is_like_clang() {
        b.define("__declspec(x)", "__attribute__((x))");
    }
    match arch.as_str() {
        "aarch64" => {
            b.file(src.join("vmxcodec_arm.cpp"));
        }
        "x86_64" => {
            b.file(src.join("vmxcodec_x86.cpp"))
                .file(src.join("vmxcodec_avx2.cpp"))
                .flag_if_supported("-mavx2")
                .flag_if_supported("-mbmi")
                .flag_if_supported("-mlzcnt")
                .flag_if_supported("-msse4.2");
        }
        other => {
            println!("cargo:warning=libvmx has no build for {other} - conformance tests will be skipped");
            println!("cargo:rustc-cfg=libvmx_missing");
            return;
        }
    }
    b.compile("vmxref");
}
