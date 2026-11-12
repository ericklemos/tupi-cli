use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let configs_workflows = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("configs")
        .join("workflows");

    println!("cargo:rerun-if-changed={}", configs_workflows.display());

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("native_workflows.rs");

    let mut entries = Vec::new();
    if let Ok(dir_entries) = fs::read_dir(&configs_workflows) {
        for entry in dir_entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("ron") {
                println!("cargo:rerun-if-changed={}", path.display());
                let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
                let abs_path = path.canonicalize().unwrap_or(path);
                entries.push((stem, abs_path));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut code = String::from(
        "/// Workflows nativos gerados em tempo de compilação a partir de configs/workflows/*.ron\n",
    );
    code.push_str("pub const NATIVE: &[(&str, &str)] = &[\n");
    for (stem, abs_path) in &entries {
        let path_str = abs_path.to_str().unwrap().replace('\\', "/");
        code.push_str(&format!("    ({stem:?}, include_str!({path_str:?})),\n"));
    }
    code.push_str("];\n");

    fs::write(&dest_path, code).unwrap();
}
