mod cargo;
mod manifest;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use manifest::{Board, TestRunner, load_all, load_board};
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
    Build {
        #[arg(long)]
        board: BoardSel,
        #[arg(long, default_value_t = true)]
        release: bool,
        /// Override the package built (default: the board's `package`).
        #[arg(long)]
        package: Option<String>,
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
        println!(
            "{:<12} target={:<35} features={}",
            b.name,
            b.target,
            b.features.join(",")
        );
    }
    Ok(())
}

fn cmd_check(sh: &Shell, root: &Path, sel: BoardSel) -> Result<()> {
    for b in boards_for(&sel, root)? {
        eprintln!("==> check {}", b.name);
        let pkg = b.package.clone();
        cargo::run_cargo(sh, root, &b, "check", &pkg, true, &[])?;
    }
    Ok(())
}

fn cmd_build(
    sh: &Shell,
    root: &Path,
    sel: BoardSel,
    release: bool,
    package: Option<String>,
) -> Result<()> {
    for b in boards_for(&sel, root)? {
        eprintln!("==> build {}", b.name);
        let pkg = package.clone().unwrap_or_else(|| b.package.clone());
        cargo::run_cargo(sh, root, &b, "build", &pkg, release, &[])?;
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
        cargo::run_cargo(sh, root, &b, "test", &pkg, true, &extra)?;
    }
    Ok(())
}

fn resolve_example(root: &Path, board: &Board, slug: &str) -> Result<String> {
    let example_dir = root.join(&board.examples_dir).join(slug);
    let cargo_toml = example_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        bail!(
            "example '{slug}' not found at {} (expected {})",
            example_dir.display(),
            cargo_toml.display()
        );
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
    Ok(slug.to_string())
}

fn cmd_run(sh: &Shell, root: &Path, board: &str, example: &str, release: bool) -> Result<()> {
    let board = load_board(root, board)?;
    if board.runner.is_none() && board.target != "x86_64-unknown-linux-gnu" {
        bail!("board {} has no [runner] config", board.name);
    }
    let pkg = resolve_example(root, &board, example)?;
    eprintln!("==> run {} :: {}", board.name, pkg);
    cargo::run_cargo(sh, root, &board, "run", &pkg, release, &[])?;
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
        } => cmd_build(&sh, &root, board, release, package),
        Cmd::Test {
            board,
            filter,
            include_hw,
        } => cmd_test(&sh, &root, board, filter, include_hw),
        Cmd::Run {
            board,
            example,
            release,
        } => cmd_run(&sh, &root, &board, &example, release),
        Cmd::Flash { board, example } => cmd_flash(&sh, &root, &board, &example),
    }
}
