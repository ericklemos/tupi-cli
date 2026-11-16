//! Build script do fork: compila o llama.cpp do submódulo (CPU por default,
//! GPU só sob feature) e gera os bindings que `src/lib.rs` inclui.

use std::env;
use std::path::PathBuf;

/// Tag fixada do llama.cpp. Fonte única: [`LLAMA_CPP_TAG`](../LLAMA_CPP_TAG).
const PINNED_TAG: &str = include_str!("LLAMA_CPP_TAG");

fn main() {
    println!("cargo:rerun-if-changed=LLAMA_CPP_TAG");
    println!("cargo:rerun-if-changed=wrapper.h");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source = manifest.join("llama.cpp");
    let pinned = PINNED_TAG.trim();

    if !source.join("CMakeLists.txt").is_file() {
        panic!(
            "submódulo llama.cpp ausente em {}: rode `git submodule update --init llama.cpp` (pin {pinned})",
            source.display()
        );
    }

    let mut build = cmake::Config::new(&source);
    build
        .define("LLAMA_BUILD_TESTS", "OFF")
        .define("LLAMA_BUILD_EXAMPLES", "OFF")
        .define("LLAMA_BUILD_SERVER", "OFF")
        .define("LLAMA_BUILD_TOOLS", "OFF")
        .define("LLAMA_CURL", "OFF")
        .define("BUILD_SHARED_LIBS", "OFF")
        // Sem `-march=native` no default: o binário precisa rodar na máquina de
        // quem usa, não só na de quem compilou.
        .define("GGML_NATIVE", "OFF");

    feature_flag(&mut build, "cuda", "GGML_CUDA");
    feature_flag(&mut build, "vulkan", "GGML_VULKAN");
    feature_flag(&mut build, "metal", "GGML_METAL");
    feature_flag(&mut build, "openmp", "GGML_OPENMP");

    let dst = build.build();

    let lib = dst.join("lib");
    let search = if lib.is_dir() { lib } else { dst.join("build") };
    println!("cargo:rustc-link-search=native={}", search.display());
    for dep in ["llama", "ggml", "ggml-base", "ggml-cpu"] {
        println!("cargo:rustc-link-lib=static={dep}");
    }
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=framework=Accelerate");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
    println!("cargo:rustc-link-lib=dylib=stdc++");

    let include = source.join("include");
    let ggml_include = source.join("ggml/include");
    let bindings = bindgen::Builder::default()
        .header(manifest.join("wrapper.h").to_string_lossy())
        .clang_arg(format!("-I{}", include.display()))
        .clang_arg(format!("-I{}", ggml_include.display()))
        .allowlist_function("llama_.*")
        .allowlist_function("ggml_.*")
        .allowlist_type("llama_.*")
        .allowlist_type("ggml_.*")
        .allowlist_var("LLAMA_.*")
        .allowlist_var("GGML_.*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen não conseguiu gerar os bindings do llama.cpp");

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("bindings.rs");
}

/// Liga um `GGML_*` do CMake quando a feature homônima está ativa. O default é
/// sempre desligado.
fn feature_flag(build: &mut cmake::Config, feature: &str, define: &str) {
    let key = format!("CARGO_FEATURE_{}", feature.to_uppercase());
    if env::var_os(key).is_some() {
        build.define(define, "ON");
    }
}
