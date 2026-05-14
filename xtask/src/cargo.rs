use crate::manifest::Board;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;
use xshell::{Shell, cmd};

/// Translate a board manifest into the environment variables cargo respects:
/// - `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` for linker script + extra rustflags
/// - `CARGO_TARGET_<TRIPLE>_RUNNER` for the qemu / probe-rs / etc. runner
/// - any `[env]` keys verbatim
pub fn project_env(board: &Board) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let triple_key = board.target.replace('-', "_").to_uppercase();

    if let Some(linker) = &board.linker {
        let mut flags: Vec<String> = Vec::new();
        if let Some(script) = &linker.script {
            flags.push(format!("-Clink-arg=-T{script}"));
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

pub fn run_cargo(
    sh: &Shell,
    workspace_root: &Path,
    board: &Board,
    sub: &str,
    package: &str,
    release: bool,
    extra: &[String],
) -> Result<()> {
    let env = project_env(board);
    let features = board.features.join(",");
    let mut args: Vec<String> = vec![
        sub.into(),
        "-p".into(),
        package.into(),
        "--target".into(),
        board.target.clone(),
        "--features".into(),
        features,
    ];
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
