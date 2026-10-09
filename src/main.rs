use clap::{Parser, Subcommand};
use git2::{ApplyLocation, Diff, FileMode, Repository, RepositoryOpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Component, Path, PathBuf},
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    version,
    about = "Apply ordered Git patches to the current working directory"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Apply { input: PathBuf },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    patches: Vec<String>,
}

#[derive(Serialize)]
struct Outcome {
    success: bool,
    patches_applied: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_patch: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Outcome {
    fn failure(applied: usize, failed_patch: Option<usize>, error: String) -> Self {
        Self {
            success: false,
            patches_applied: applied,
            failed_patch,
            error: Some(error),
        }
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
            return ExitCode::SUCCESS;
        }
        Err(error) => return emit(Outcome::failure(0, None, error.to_string())),
    };
    let Command::Apply { input } = cli.command;
    emit(match run(&input) {
        Ok(outcome) => outcome,
        Err(error) => Outcome::failure(0, None, error),
    })
}

fn emit(outcome: Outcome) -> ExitCode {
    println!(
        "{}",
        serde_json::to_string(&outcome).expect("outcome is serializable")
    );
    if outcome.success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run(input_path: &Path) -> Result<Outcome, String> {
    let bytes = fs::read(input_path).map_err(|e| format!("Cannot read input: {e}"))?;
    let input: Input =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid input JSON: {e}"))?;
    let root = env::current_dir()
        .and_then(fs::canonicalize)
        .map_err(|e| format!("Cannot resolve current directory: {e}"))?;
    let repo = Repository::open_ext(
        &root,
        RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&Path>(),
    )
    .map_err(|e| format!("Current directory must be a Git repository root: {e}"))?;
    let workdir = repo
        .workdir()
        .ok_or("Current directory must be a non-bare Git repository root")?;
    if fs::canonicalize(workdir).map_err(|e| e.to_string())? != root {
        return Err("Current directory must be the Git repository root".into());
    }

    for (index, patch) in input.patches.iter().enumerate() {
        if let Err(error) = apply_patch(&repo, &root, patch) {
            return Ok(Outcome::failure(index, Some(index), error));
        }
    }
    Ok(Outcome {
        success: true,
        patches_applied: input.patches.len(),
        failed_patch: None,
        error: None,
    })
}

fn apply_patch(repo: &Repository, root: &Path, patch: &str) -> Result<(), String> {
    // libgit2 squashes repeated slashes before exposing paths. Reject these
    // spellings in file headers rather than silently changing their meaning.
    for line in patch.lines().filter(|line| line.starts_with("diff --git ")) {
        let header = &line[11..];
        if header.contains("//")
            || header.starts_with('/')
            || header.starts_with("\"/")
            || header.contains(" /")
            || header.contains(" \"/")
        {
            return Err("Unsafe absolute or repeated-separator patch path".into());
        }
    }
    let diff =
        Diff::from_buffer(patch.as_bytes()).map_err(|e| format!("Invalid Git patch: {e}"))?;
    if diff.deltas().len() == 0 {
        return Err("Patch contains no file changes".into());
    }
    // Validate every destination and source before any part of this patch is applied.
    for delta in diff.deltas() {
        for file in [delta.old_file(), delta.new_file()] {
            if matches!(file.mode(), FileMode::Link | FileMode::Commit) {
                return Err("Symlink and submodule patches are not supported".into());
            }
            if let Some(path) = file.path() {
                validate_path(root, path)?;
            }
        }
    }
    repo.apply(&diff, ApplyLocation::WorkDir, None)
        .map_err(|e| format!("Patch could not be applied: {e}"))
}

fn validate_path(root: &Path, path: &Path) -> Result<(), String> {
    let unsafe_path = || format!("Unsafe patch path: {}", path.display());
    if path.as_os_str().is_empty() {
        return Err(unsafe_path());
    }
    let mut resolved = root.to_path_buf();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(unsafe_path());
        };
        // Also reject Windows separators, drive/stream syntax and metadata aliases on Unix.
        let text = name.to_string_lossy();
        if text.contains(['\\', ':'])
            || text.ends_with(['.', ' '])
            || text.eq_ignore_ascii_case(".git")
            || text.eq_ignore_ascii_case("git~1")
        {
            return Err(unsafe_path());
        }
        resolved.push(name);
        match fs::symlink_metadata(&resolved) {
            Ok(metadata) if is_symlink_or_reparse_point(&metadata) => {
                return Err(format!(
                    "Patch path traverses a symlink or reparse point: {}",
                    path.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Cannot inspect patch path {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn is_symlink_or_reparse_point(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
