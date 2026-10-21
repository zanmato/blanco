use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

/// Inflates a committed `.br` blob into OUT_DIR, since a build script must not
/// write into the source tree.
fn inflate(src_dir: &Path, out_dir: &Path, file: &str) -> Result<PathBuf, Box<dyn Error>> {
    let blob_path = src_dir.join(format!("{file}.br"));
    println!("cargo:rerun-if-changed={}", blob_path.display());

    let compressed = fs::read(&blob_path).map_err(|error| {
        format!(
            "missing {}: {error} (run scripts/generate-parser.sh)",
            blob_path.display()
        )
    })?;
    let mut inflated = Vec::new();
    brotli::BrotliDecompress(&mut compressed.as_slice(), &mut inflated)
        .map_err(|error| format!("failed to inflate {}: {error}", blob_path.display()))?;

    let out_path = out_dir.join(file);
    fs::write(&out_path, inflated)?;
    Ok(out_path)
}

fn main() -> Result<(), Box<dyn Error>> {
    let src_dir = Path::new("src");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);

    let parser_path = inflate(src_dir, &out_dir, "parser.c")?;
    inflate(src_dir, &out_dir, "node-types.json")?;

    let mut c_config = cc::Build::new();
    c_config.std("c11").include(src_dir).file(&parser_path);

    #[cfg(target_env = "msvc")]
    c_config.flag("-utf-8");

    c_config.compile("tree-sitter-redis");
    Ok(())
}
