use clap::{Parser, Subcommand};
use git2::{ApplyLocation, Diff, FileMode, Repository, RepositoryOpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    io::{self, Read},
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
struct PatchInput {
    patches: Vec<PatchSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationInput {
    operations: Vec<Operation>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Input {
    Patches(PatchInput),
    Operations(OperationInput),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Patch { source: PatchSource },
    CreateFile { path: PathBuf, content: String },
    DeleteFile { path: PathBuf },
    MoveFile { from: PathBuf, to: PathBuf },
    CreateDirectory { path: PathBuf },
    DeleteDirectory { path: PathBuf },
    MoveDirectory { from: PathBuf, to: PathBuf },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PatchSource {
    LegacyInline(String),
    Typed(TypedPatch),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum TypedPatch {
    Inline { patch: String },
    File { path: PathBuf },
}

impl PatchSource {
    fn read(&self) -> Result<String, String> {
        match self {
            Self::LegacyInline(patch) | Self::Typed(TypedPatch::Inline { patch }) => {
                Ok(patch.clone())
            }
            Self::Typed(TypedPatch::File { path }) => fs::read_to_string(path)
                .map_err(|e| format!("Cannot read patch file {}: {e}", path.display())),
        }
    }
}

#[derive(Serialize)]
struct Outcome {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    patches_applied: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operations_applied: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_patch: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_operation: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Outcome {
    fn failure(applied: usize, failed_patch: Option<usize>, error: String) -> Self {
        Self {
            success: false,
            patches_applied: Some(applied),
            operations_applied: None,
            failed_patch,
            failed_operation: None,
            error: Some(error),
        }
    }

    fn operation_failure(applied: usize, failed_operation: usize, error: String) -> Self {
        Self {
            success: false,
            patches_applied: None,
            operations_applied: Some(applied),
            failed_patch: None,
            failed_operation: Some(failed_operation),
            error: Some(error),
        }
    }

    fn success(applied: usize, operations: bool) -> Self {
        Self {
            success: true,
            patches_applied: (!operations).then_some(applied),
            operations_applied: operations.then_some(applied),
            failed_patch: None,
            failed_operation: None,
            error: None,
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
    let bytes = if input_path == Path::new("-") {
        let mut bytes = Vec::new();
        io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read stdin: {e}"))?;
        bytes
    } else {
        fs::read(input_path).map_err(|e| format!("Cannot read input: {e}"))?
    };
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

    match input {
        Input::Patches(input) => {
            for (index, source) in input.patches.iter().enumerate() {
                if let Err(error) = source
                    .read()
                    .and_then(|patch| apply_patch(&repo, &root, &patch))
                {
                    return Ok(Outcome::failure(index, Some(index), error));
                }
            }
            Ok(Outcome::success(input.patches.len(), false))
        }
        Input::Operations(input) => {
            for (index, operation) in input.operations.iter().enumerate() {
                if let Err(error) = apply_operation(&repo, &root, operation) {
                    return Ok(Outcome::operation_failure(index, index, error));
                }
            }
            Ok(Outcome::success(input.operations.len(), true))
        }
    }
}

fn apply_operation(repo: &Repository, root: &Path, operation: &Operation) -> Result<(), String> {
    match operation {
        Operation::Patch { source } => {
            let patch = source.read()?;
            apply_patch(repo, root, &patch)
        }
        Operation::CreateFile { path, content } => {
            let path = safe_path(root, path)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| format!("Cannot create file {}: {e}", path.display()))?;
            file.write_all(content.as_bytes())
                .map_err(|e| format!("Cannot write file {}: {e}", path.display()))
        }
        Operation::DeleteFile { path } => {
            let path = safe_path(root, path)?;
            require_kind(&path, false)?;
            fs::remove_file(&path)
                .map_err(|e| format!("Cannot delete file {}: {e}", path.display()))
        }
        Operation::MoveFile { from, to } => move_path(root, from, to, false),
        Operation::CreateDirectory { path } => {
            let path = safe_path(root, path)?;
            fs::create_dir(&path)
                .map_err(|e| format!("Cannot create directory {}: {e}", path.display()))
        }
        Operation::DeleteDirectory { path } => {
            let path = safe_path(root, path)?;
            require_kind(&path, true)?;
            fs::remove_dir(&path)
                .map_err(|e| format!("Cannot delete directory {}: {e}", path.display()))
        }
        Operation::MoveDirectory { from, to } => move_path(root, from, to, true),
    }
}

fn safe_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    validate_path(root, relative)?;
    Ok(root.join(relative))
}

fn require_kind(path: &Path, directory: bool) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("Cannot inspect {}: {e}", path.display()))?;
    if metadata.is_dir() == directory && (directory || metadata.is_file()) {
        Ok(())
    } else {
        Err(format!("Unexpected file type: {}", path.display()))
    }
}

fn move_path(root: &Path, from: &Path, to: &Path, directory: bool) -> Result<(), String> {
    let source = safe_path(root, from)?;
    let target = safe_path(root, to)?;
    require_kind(&source, directory)?;
    match fs::symlink_metadata(&target) {
        Ok(_) => return Err(format!("Destination already exists: {}", target.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Cannot inspect destination {}: {e}",
                target.display()
            ));
        }
    }
    fs::rename(&source, &target).map_err(|e| {
        format!(
            "Cannot move {} to {}: {e}",
            source.display(),
            target.display()
        )
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
            #[cfg(windows)]
            if let Some(bytes) = file.path_bytes() {
                std::str::from_utf8(bytes).map_err(|_| "Invalid UTF-8 patch path on Windows")?;
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
    if path.as_os_str().is_empty() || path.as_os_str().to_string_lossy().contains('\\') {
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
