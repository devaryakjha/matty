//! Joins each language's Fluent files under `locales/<tag>/` into one
//! resource, `$OUT_DIR/locales/<tag>.ftl`, for `i18n` to include. One file
//! per area keeps parallel work from touching the same file.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=locales");
    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("locales");
    std::fs::create_dir_all(&out).expect("create the joined locales folder");
    let languages = std::fs::read_dir("locales").expect("read locales/");
    for language in languages.flatten() {
        if !language.path().is_dir() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", language.path().display());
        let mut files: Vec<_> = std::fs::read_dir(language.path())
            .expect("read a language's folder")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "ftl"))
            .collect();
        files.sort();
        let mut joined = String::new();
        for file in files {
            println!("cargo:rerun-if-changed={}", file.display());
            joined.push_str(&std::fs::read_to_string(&file).expect("read a Fluent file"));
            joined.push('\n');
        }
        let mut name = language.file_name();
        name.push(".ftl");
        std::fs::write(out.join(name), joined).expect("write the joined Fluent file");
    }
}
