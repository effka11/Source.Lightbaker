use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let sdk = find_sdk();
    let lib = sdk.join("lib");
    let bin = sdk.join("bin");
    if !lib.join("embree4.lib").is_file() {
        panic!("embree4.lib is missing in {}", lib.display());
    }

    println!("cargo:rerun-if-env-changed=EMBREE_DIR");
    println!("cargo:rerun-if-changed={}", bin.display());
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=dylib=embree4");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let profile_dir = out_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("profile dir")
        .to_path_buf();
    copy_dlls(&bin, &profile_dir);
    copy_dlls(&bin, &profile_dir.join("deps"));
}

fn find_sdk() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(dir) = env::var("EMBREE_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    candidates.push(manifest.join("../../third_party/embree"));

    for candidate in &candidates {
        if candidate.join("lib/embree4.lib").is_file() {
            return candidate.clone();
        }
    }

    panic!(
        "Embree 4.4.1 not found. Set EMBREE_DIR or unpack \
         embree-4.4.1.x64.windows.zip (not the SYCL build) into third_party/embree. \
         https://github.com/RenderKit/embree/releases/download/v4.4.1/embree-4.4.1.x64.windows.zip"
    );
}

fn copy_dlls(bin: &Path, dest: &Path) {
    fs::create_dir_all(dest).unwrap_or_else(|err| panic!("create {}: {err}", dest.display()));
    let entries = fs::read_dir(bin).unwrap_or_else(|err| panic!("read {}: {err}", bin.display()));
    for entry in entries {
        let entry = entry.expect("dll entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("dll") {
            continue;
        }
        let target = dest.join(path.file_name().expect("dll name"));
        fs::copy(&path, &target)
            .unwrap_or_else(|err| panic!("copy {} -> {}: {err}", path.display(), target.display()));
    }
}
