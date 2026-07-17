use clap::Parser;
use compact_codegen::generate_from_file;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "midnight-rust-bindgen",
    about = "Generate Rust bindings from Compact contract-info.json",
    version
)]
struct Cli {
    /// Path to contract-info.json
    #[arg(short, long)]
    input: PathBuf,

    /// Output directory (will contain Cargo.toml + src/lib.rs)
    #[arg(short, long)]
    output: PathBuf,

    /// Contract name (e.g., Gateway)
    #[arg(short, long, default_value = "Contract")]
    name: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let generated = generate_from_file(&cli.input, &cli.name)?;

    // Write crate structure
    let src_dir = cli.output.join("src");
    std::fs::create_dir_all(&src_dir)?;
    std::fs::write(cli.output.join("Cargo.toml"), &generated.cargo_toml)?;
    std::fs::write(src_dir.join("lib.rs"), &generated.lib_rs)?;

    eprintln!(
        "Generated crate at {} ({} bytes)",
        cli.output.display(),
        generated.cargo_toml.len() + generated.lib_rs.len()
    );
    Ok(())
}
