//! With `--features bundle-native`, compresses the native Foundry engine from `native/`
//! into the executable; `install::unpack_native` unpacks it at runtime.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "bundle-native")]
    bundle_native();
}

#[cfg(feature = "bundle-native")]
fn bundle_native() {
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::path::PathBuf;
    use std::{env, fs};

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let native = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("native");
    println!("cargo:rerun-if-changed={}", native.display());

    let ext = if env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" { "dll" } else { "so" };
    let mut libs: Vec<PathBuf> = fs::read_dir(&native)
        .expect("native/ missing – run scripts/fetch-native-nightly.sh first")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == ext))
        .collect();
    libs.sort();
    assert!(!libs.is_empty(), "no .{ext} files in {}", native.display());

    let mut hasher = DefaultHasher::new();
    let mut files = String::new();
    for path in libs {
        let name = path.file_name().unwrap().to_str().unwrap();
        let data = fs::read(&path).unwrap();
        (name, &data).hash(&mut hasher);
        let compressed = out.join(format!("{name}.zst"));
        fs::write(&compressed, zstd::encode_all(data.as_slice(), 10).unwrap()).unwrap();
        files += &format!("    ({name:?}, include_bytes!({compressed:?})),\n");
    }

    let code = format!(
        "/// Identifies this set of libraries; names the directory they are unpacked to.\n\
         pub const VERSION: &str = \"{:016x}\";\n\
         pub const FILES: &[(&str, &[u8])] = &[\n{files}];\n",
        hasher.finish()
    );
    fs::write(out.join("bundled_native.rs"), code).unwrap();
}
