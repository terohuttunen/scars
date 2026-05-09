use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Board {
    pub name: String,
    pub target: String,
    pub package: String,
    pub features: Vec<String>,
    pub test_runner: TestRunner,
    pub examples_dir: PathBuf,
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
    pub script: String,
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
