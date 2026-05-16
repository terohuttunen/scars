use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Board {
    pub name: String,
    pub target: String,
    pub package: String,
    /// Features always forwarded — to the kernel crate (`check`,
    /// `test`, `bench`) and to example crates (`run`, `flash`).
    /// Shared examples must declare these in their own `[features]`
    /// table; cargo errors loudly on mismatch.
    pub features: Vec<String>,
    /// Features added only when targeting the kernel crate. Used for
    /// kernel-internal flags (e.g. `bench-large`, which gates
    /// memory-heavy `[[bench]]` entries via `required-features`)
    /// that have no meaning for example crates.
    #[serde(default)]
    pub kernel_features: Vec<String>,
    pub test_runner: TestRunner,
    pub examples_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub linker: Option<Linker>,
    #[serde(default)]
    pub runner: Option<Runner>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TestRunner {
    Native,
    Qemu,
    #[serde(rename = "probe-rs")]
    ProbeRs,
}

#[derive(Debug, Deserialize)]
pub struct Linker {
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub rustflags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Runner {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

pub fn load_board(workspace_root: &Path, name: &str) -> Result<Board> {
    let path = workspace_root.join("boards").join(format!("{name}.toml"));
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let board: Board =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if board.name != name {
        return Err(anyhow!(
            "board file {} declares name = {:?}, expected {:?}",
            path.display(),
            board.name,
            name
        ));
    }
    Ok(board)
}

/// One `[[bench]]` stanza from a package's Cargo.toml. Used by
/// `cmd_bench` to drive per-bench `cargo test --bench NAME` invocations.
#[derive(Debug, Deserialize)]
pub struct BenchEntry {
    pub name: String,
    #[serde(default, rename = "required-features")]
    pub required_features: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PackageManifest {
    #[serde(default)]
    bench: Vec<BenchEntry>,
}

/// Read the `[[bench]]` stanzas from the given workspace package's
/// `Cargo.toml`. Returns an empty vec if there are no benches declared.
pub fn load_bench_entries(workspace_root: &Path, package: &str) -> Result<Vec<BenchEntry>> {
    let path = workspace_root.join(package).join("Cargo.toml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: PackageManifest =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(manifest.bench)
}

pub fn load_all(workspace_root: &Path) -> Result<Vec<Board>> {
    let dir = workspace_root.join("boards");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("toml") {
                p.file_stem().and_then(|s| s.to_str()).map(String::from)
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
        .iter()
        .map(|n| load_board(workspace_root, n))
        .collect()
}
