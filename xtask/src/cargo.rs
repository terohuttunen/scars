use crate::manifest::Board;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;
use xshell::{Shell, cmd};

/// Translate a board manifest into the environment variables cargo respects:
/// - `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` for linker script + extra rustflags
/// - `CARGO_TARGET_<TRIPLE>_RUNNER` for the qemu / probe-rs / etc. runner
/// - any `[env]` keys verbatim
///
/// The linker script path is resolved against `workspace_root` so the
/// flag works regardless of where rust-lld is ultimately invoked from
/// (notably: example crates outside the workspace run with their own
/// build CWD, not the workspace root).
pub fn project_env(board: &Board, workspace_root: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let triple_key = board.target.replace('-', "_").to_uppercase();

    if let Some(linker) = &board.linker {
        let mut flags: Vec<String> = Vec::new();
        if let Some(script) = &linker.script {
            let script_path = workspace_root.join(script);
            flags.push(format!("-Clink-arg=-T{}", script_path.display()));
        }
        flags.extend(linker.rustflags.iter().cloned());
        if !flags.is_empty() {
            // CARGO_TARGET_<TRIPLE>_RUSTFLAGS is split on whitespace; flags must not contain spaces.
            env.insert(
                format!("CARGO_TARGET_{triple_key}_RUSTFLAGS"),
                flags.join(" "),
            );
        }
    }

    if let Some(r) = &board.runner {
        let mut parts = Vec::with_capacity(1 + r.args.len());
        parts.push(r.program.clone());
        parts.extend(r.args.iter().cloned());
        env.insert(format!("CARGO_TARGET_{triple_key}_RUNNER"), parts.join(" "));
    }

    for (k, v) in &board.env {
        env.insert(k.clone(), v.clone());
    }

    env
}

/// How to point cargo at the target crate. Workspace members (the
/// `scars` lib, in-workspace examples) come in by `-p name`; standalone
/// example crates that live outside the workspace come in by
/// `--manifest-path Cargo.toml` so cargo can find them without a
/// matching workspace member.
pub enum CargoTarget<'a> {
    Package(&'a str),
    Manifest(&'a Path),
}

/// Drive a cargo subcommand against `target`, forwarding the
/// explicit `features` list (caller decides whether to use
/// `board.features` only or the union with `board.kernel_features`).
pub fn run_cargo(
    sh: &Shell,
    workspace_root: &Path,
    board: &Board,
    sub: &str,
    target: CargoTarget<'_>,
    release: bool,
    extra: &[String],
    features: &[String],
) -> Result<()> {
    let env = project_env(board, workspace_root);
    let features = features.join(",");
    let mut args: Vec<String> = vec![sub.into()];
    match target {
        CargoTarget::Package(name) => {
            args.push("-p".into());
            args.push(name.into());
        }
        CargoTarget::Manifest(path) => {
            args.push("--manifest-path".into());
            args.push(path.display().to_string());
        }
    }
    args.push("--target".into());
    args.push(board.target.clone());
    args.push("--features".into());
    args.push(features);
    if release {
        args.push("--release".into());
    }
    args.extend(extra.iter().cloned());

    let _dir = sh.push_dir(workspace_root);
    let mut command = cmd!(sh, "cargo {args...}");
    for (k, v) in &env {
        command = command.env(k, v);
    }
    command
        .run()
        .with_context(|| format!("cargo {sub} failed for board {}", board.name))?;
    Ok(())
}
