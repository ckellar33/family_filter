extern crate bindgen;

use bindgen::Abi;
use std::env;
use std::path::PathBuf;

// Cargo's `aarch64-apple-ios-sim` target triple uses a Rust-only "-sim"
// environment suffix that isn't a triple Clang/libclang understand (Clang
// wants the full "-simulator" spelling). Left uncorrected, bindgen forwards
// the raw triple to clang, which rejects it outright and then falls back to
// a sysroot that doesn't contain the iOS headers at all (hence the
// "dns_sd.h"/"dispatch.h" not found errors alongside the invalid-triple one).
fn ios_clang_target_and_sysroot() -> Option<(String, String)> {
    let target_os = env::var("CARGO_CFG_TARGET_OS").ok()?;
    if target_os != "ios" {
        return None;
    }
    let is_sim = env::var("CARGO_CFG_TARGET_ABI").map(|abi| abi == "sim").unwrap_or(false);
    let sdk = if is_sim { "iphonesimulator" } else { "iphoneos" };
    let target = env::var("TARGET").ok()?;
    let clang_target = if is_sim {
        target.trim_end_matches("-sim").to_string() + "-simulator"
    } else {
        target
    };
    let sdk_path = std::process::Command::new("xcrun")
        .args(["--sdk", sdk, "--show-sdk-path"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    Some((clang_target, sdk_path))
}

fn main() {
    use std::io::{Read, Seek, Write};
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();

    let bindings_rs_path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bindings.rs");
    let ios_fix = ios_clang_target_and_sysroot();

    let mut builder = bindgen::Builder::default();
    if let Some((clang_target, sysroot)) = &ios_fix {
        builder = builder
            .clang_arg(format!("--target={clang_target}"))
            .clang_arg("-isysroot")
            .clang_arg(sysroot);
    }

    // Generate the dns_sd.h bindings first so we can
    // add some marks eg. #[link] and abi.
    builder
        .header("wrapper.h")
        .ctypes_prefix("::libc")
        .allowlist_file(".*dns_sd.h")
        .allowlist_recursively(false)
        .trust_clang_mangling(false)
        .override_abi(Abi::System, "DNSService.*")
        .override_abi(Abi::System, "TXTRecord.*")
        .generate()
        .expect("failed to generate dns_sd.h bindings")
        .write_to_file(&bindings_rs_path)
        .expect("failed to write dns_sd.h bindings to file");

    if target_os == "windows" {
        // On Windows, we need to mark the functions as being available in dnssd.dll
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(&bindings_rs_path)
            .unwrap();

        let mut contents: String = String::new();

        file.read_to_string(&mut contents).unwrap();

        if target_arch == "x86" {
            // Decorated names are used by default on Win x86,
            // so we need to specify import_name_type = "decorated"
            contents = contents.replace(
                "extern \"system\" {",
                "#[link(name = \"dnssd\", kind = \"raw-dylib\", import_name_type = \"undecorated\")]\r\nextern \"system\" {",
            );
        } else {
            contents = contents.replace(
                "extern \"system\" {",
                "#[link(name = \"dnssd\", kind = \"raw-dylib\")]\r\nextern \"system\" {",
            );
        }

        file.seek(std::io::SeekFrom::Start(0)).unwrap();
        file.set_len(0).unwrap();
        file.write_all(contents.as_bytes())
            .expect("Unable to write data");
    }

    // Generate bindings for everything else
    // Certain windows types we don't need have some issues with rust & bindgen
    // Further discussion: https://github.com/rust-lang/rust-bindgen/issues/1562
    let mut builder2 = bindgen::Builder::default();
    if let Some((clang_target, sysroot)) = &ios_fix {
        builder2 = builder2
            .clang_arg(format!("--target={clang_target}"))
            .clang_arg("-isysroot")
            .clang_arg(sysroot);
    }
    builder2
        .header("wrapper.h")
        .ctypes_prefix("::libc")
        .blocklist_file(".*dns_sd.h")
        .blocklist_type("IMAGE_TLS_DIRECTORY")
        .blocklist_type("PIMAGE_TLS_DIRECTORY")
        .blocklist_type("IMAGE_TLS_DIRECTORY64")
        .blocklist_type("PIMAGE_TLS_DIRECTORY64")
        .blocklist_type("_IMAGE_TLS_DIRECTORY64")
        .generate()
        .expect("failed to generate system bindings")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bindings2.rs"))
        .expect("failed to write system bindings to file");
}
