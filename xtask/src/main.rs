mod cargo;
mod manifest;

use anyhow::{Context, Result, anyhow, bail};
use cargo::CargoTarget;
use clap::{Parser, Subcommand};
use manifest::{Board, TestRunner, load_all, load_bench_entries, load_board};
use std::path::{Path, PathBuf};
use xshell::Shell;

#[derive(Parser)]
#[command(about = "SCARS build orchestration")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List configured boards.
    Boards,
    /// `cargo check` for one or all boards.
    Check {
        #[arg(long)]
        board: BoardSel,
    },
    /// `cargo build` for one or all boards.
    ///
    /// With `--example NAME`, builds a specific example crate against
    /// the board (does not run it). Useful for producing an ELF you
    /// hand to external tools like `picotool`. `--example` requires
    /// a specific `--board` (not `all`) and is mutually exclusive
    /// with `--package`.
    Build {
        #[arg(long)]
        board: BoardSel,
        #[arg(long, default_value_t = true)]
        release: bool,
        /// Override the package built (default: the board's `package`).
        #[arg(long, conflicts_with = "example")]
        package: Option<String>,
        /// Build a specific example for this board. Produces the
        /// example's ELF without invoking the board's runner.
        #[arg(long)]
        example: Option<String>,
    },
    /// `cargo test` for one or all boards. probe-rs boards skipped unless --include-hw.
    Test {
        #[arg(long)]
        board: BoardSel,
        /// Optional test name filter passed through to cargo test.
        filter: Option<String>,
        /// Include hardware (probe-rs) boards in `--board all`.
        #[arg(long)]
        include_hw: bool,
    },
    /// `cargo bench` for one or all boards. probe-rs boards skipped unless --include-hw.
    Bench {
        #[arg(long)]
        board: BoardSel,
        /// Optional bench name (e.g. "sched") — selects a single `[[bench]]` target.
        filter: Option<String>,
        /// Include hardware (probe-rs) boards in `--board all`.
        #[arg(long)]
        include_hw: bool,
    },
    /// `cargo run` for a single board+example.
    Run {
        #[arg(long)]
        board: String,
        #[arg(long)]
        example: String,
        #[arg(long, default_value_t = true)]
        release: bool,
    },
    /// Build and flash a single board+example. Currently same as `run --release`.
    Flash {
        #[arg(long)]
        board: String,
        #[arg(long)]
        example: String,
    },
}

#[derive(Clone, Debug)]
enum BoardSel {
    One(String),
    All,
}

impl std::str::FromStr for BoardSel {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(if s == "all" {
            BoardSel::All
        } else {
            BoardSel::One(s.to_string())
        })
    }
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest_dir)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("xtask must live one directory below the workspace root"))
}

fn boards_for(sel: &BoardSel, root: &Path) -> Result<Vec<Board>> {
    match sel {
        BoardSel::One(name) => Ok(vec![load_board(root, name)?]),
        BoardSel::All => load_all(root),
    }
}

fn cmd_boards(root: &Path) -> Result<()> {
    let boards = load_all(root)?;
    for b in boards {
        if b.kernel_features.is_empty() {
            println!(
                "{:<12} target={:<35} features={}",
                b.name,
                b.target,
                b.features.join(","),
            );
        } else {
            println!(
                "{:<12} target={:<35} features={} kernel={}",
                b.name,
                b.target,
                b.features.join(","),
                b.kernel_features.join(","),
            );
        }
    }
    Ok(())
}

/// Union of `board.features` and `board.kernel_features`. Used by
/// commands that target the kernel crate; `bench-large` and similar
/// kernel-only flags live in `kernel_features` and only activate on
/// these paths.
fn kernel_feature_union(b: &Board) -> Vec<String> {
    let mut v = b.features.clone();
    v.extend(b.kernel_features.iter().cloned());
    v
}

fn cmd_check(sh: &Shell, root: &Path, sel: BoardSel) -> Result<()> {
    for b in boards_for(&sel, root)? {
        eprintln!("==> check {}", b.name);
        let pkg = b.package.clone();
        let features = kernel_feature_union(&b);
        cargo::run_cargo(
            sh,
            root,
            &b,
            "check",
            CargoTarget::Package(&pkg),
            true,
            &[],
            &features,
        )?;
    }
    Ok(())
}

fn cmd_build(
    sh: &Shell,
    root: &Path,
    sel: BoardSel,
    release: bool,
    package: Option<String>,
    example: Option<String>,
) -> Result<()> {
    if let Some(example) = example {
        // Example build: resolve via the same path as `run`, but stop
        // at the link step. Pass only `board.features` (chip-* flags,
        // same as `cmd_run`); kernel-only features like `bench-large`
        // don't belong on example crates.
        let board_name = match sel {
            BoardSel::One(name) => name,
            BoardSel::All => bail!("`--example` requires a specific `--board` (not `all`)"),
        };
        let board = load_board(root, &board_name)?;
        let dir = resolve_example(root, &board, &example)?;
        eprintln!(
            "==> build {} :: {} ({})",
            board.name,
            example,
            dir.display()
        );
        let manifest = dir.join("Cargo.toml");
        cargo::run_cargo(
            sh,
            root,
            &board,
            "build",
            CargoTarget::Manifest(&manifest),
            release,
            &[],
            &board.features,
        )?;
        return Ok(());
    }
    for b in boards_for(&sel, root)? {
        eprintln!("==> build {}", b.name);
        let pkg = package.clone().unwrap_or_else(|| b.package.clone());
        let features = kernel_feature_union(&b);
        cargo::run_cargo(
            sh,
            root,
            &b,
            "build",
            CargoTarget::Package(&pkg),
            release,
            &[],
            &features,
        )?;
    }
    Ok(())
}

fn cmd_test(
    sh: &Shell,
    root: &Path,
    sel: BoardSel,
    filter: Option<String>,
    include_hw: bool,
) -> Result<()> {
    let mut boards = boards_for(&sel, root)?;
    if matches!(sel, BoardSel::All) && !include_hw {
        boards.retain(|b| b.test_runner != TestRunner::ProbeRs);
    }
    // When a filter is given, select the target via `--test NAME`
    // rather than passing it as a positional. Positional filters are
    // forwarded to the libtest harness on the runner's argv, and
    // probe-rs (test_runner = ProbeRs) rejects unknown args on
    // harness=false binaries.
    let extra: Vec<String> = match filter {
        Some(name) => vec!["--test".into(), name],
        None => Vec::new(),
    };
    for b in boards {
        eprintln!("==> test {}", b.name);
        let pkg = b.package.clone();
        let features = kernel_feature_union(&b);
        cargo::run_cargo(
            sh,
            root,
            &b,
            "test",
            CargoTarget::Package(&pkg),
            true,
            &extra,
            &features,
        )?;
    }
    Ok(())
}

fn cmd_bench(
    sh: &Shell,
    root: &Path,
    sel: BoardSel,
    filter: Option<String>,
    include_hw: bool,
) -> Result<()> {
    let mut boards = boards_for(&sel, root)?;
    if matches!(sel, BoardSel::All) && !include_hw {
        boards.retain(|b| b.test_runner != TestRunner::ProbeRs);
    }
    for b in boards {
        let entries = load_bench_entries(root, &b.package)?;
        let selected: Vec<&manifest::BenchEntry> = match filter.as_ref() {
            Some(name) => entries.iter().filter(|e| e.name == *name).collect(),
            None => entries.iter().collect(),
        };
        if selected.is_empty() {
            if let Some(name) = &filter {
                bail!("no bench named {:?} in package {}", name, b.package);
            } else {
                eprintln!("==> bench {}: no [[bench]] entries", b.name);
                continue;
            }
        }
        // Drive each bench via `cargo test --bench NAME` (not `cargo
        // bench`). cargo bench appends `--bench` to the runner argv to
        // signal libtest's bench mode — but our harness=false binaries
        // don't run libtest, and probe-rs rejects the unknown arg.
        // Using `cargo test --bench NAME` per bench also avoids cargo
        // pulling in the lib's unit-test target on every run.
        let features = kernel_feature_union(&b);
        for entry in selected {
            let unmet: Vec<&String> = entry
                .required_features
                .iter()
                .filter(|f| !features.contains(f))
                .collect();
            if !unmet.is_empty() {
                eprintln!(
                    "==> skip bench {} on {} (requires {:?})",
                    entry.name, b.name, unmet
                );
                continue;
            }
            eprintln!("==> bench {} on {}", entry.name, b.name);
            let extra = vec![
                "--bench".into(),
                entry.name.clone(),
                "--".into(),
                "--nocapture".into(),
            ];
            cargo::run_cargo(
                sh,
                root,
                &b,
                "test",
                CargoTarget::Package(&b.package),
                true,
                &extra,
                &features,
            )?;
        }
    }
    Ok(())
}

fn resolve_example(root: &Path, board: &Board, slug: &str) -> Result<PathBuf> {
    let candidates: Vec<PathBuf> = board
        .examples_dirs
        .iter()
        .map(|d| root.join(d).join(slug))
        .collect();
    for dir in &candidates {
        let cargo_toml = dir.join("Cargo.toml");
        if !cargo_toml.exists() {
            continue;
        }
        // Convention: example crate name == directory name. Sanity-check the manifest
        // so a future drift fails loud rather than silently picking the wrong crate.
        let text = std::fs::read_to_string(&cargo_toml)
            .with_context(|| format!("reading {}", cargo_toml.display()))?;
        let value: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", cargo_toml.display()))?;
        let pkg_name = value
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .ok_or_else(|| anyhow!("{} missing package.name", cargo_toml.display()))?;
        if pkg_name != slug {
            bail!(
                "example {} declares package.name = {:?}; convention requires it to match the directory ({:?})",
                cargo_toml.display(),
                pkg_name,
                slug
            );
        }
        return Ok(dir.clone());
    }
    bail!(
        "example '{slug}' not found in any of {:?}",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
    );
}

fn cmd_run(sh: &Shell, root: &Path, board: &str, example: &str, release: bool) -> Result<()> {
    let board = load_board(root, board)?;
    if board.runner.is_none() && board.target != "x86_64-unknown-linux-gnu" {
        bail!("board {} has no [runner] config", board.name);
    }
    let dir = resolve_example(root, &board, example)?;
    eprintln!("==> run {} :: {} ({})", board.name, example, dir.display());
    // Examples may live outside workspace.members (e.g. same-named
    // `tasking` per chip family), so drive them by manifest path. This
    // works equally for in-workspace and standalone example crates.
    let manifest = dir.join("Cargo.toml");
    // Strict pass-through of `board.features` only — `kernel_features`
    // (e.g. `bench-large`) stay kernel-side. Examples must declare
    // every feature in `board.features`; cargo errors loudly on
    // mismatch.
    cargo::run_cargo(
        sh,
        root,
        &board,
        "run",
        CargoTarget::Manifest(&manifest),
        release,
        &[],
        &board.features,
    )?;
    Ok(())
}

fn cmd_flash(sh: &Shell, root: &Path, board: &str, example: &str) -> Result<()> {
    let b = load_board(root, board)?;
    if b.runner.is_none() {
        bail!("board {} has no [runner] config to flash through", b.name);
    }
    cmd_run(sh, root, board, example, true)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = workspace_root()?;
    let sh = Shell::new()?;
    match cli.cmd {
        Cmd::Boards => cmd_boards(&root),
        Cmd::Check { board } => cmd_check(&sh, &root, board),
        Cmd::Build {
            board,
            release,
            package,
            example,
        } => cmd_build(&sh, &root, board, release, package, example),
        Cmd::Test {
            board,
            filter,
            include_hw,
        } => cmd_test(&sh, &root, board, filter, include_hw),
        Cmd::Bench {
            board,
            filter,
            include_hw,
        } => cmd_bench(&sh, &root, board, filter, include_hw),
        Cmd::Run {
            board,
            example,
            release,
        } => cmd_run(&sh, &root, &board, &example, release),
        Cmd::Flash { board, example } => cmd_flash(&sh, &root, &board, &example),
    }
}
