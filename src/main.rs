use clap::{Parser, Subcommand};
use executor::{Outcome, apply_json};
use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    version,
    about = "Apply ordered patches and file operations to the current working directory"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Apply { input: PathBuf },
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
        Err(error) => return emit(Outcome::failure(error.to_string())),
    };
    let Command::Apply { input } = cli.command;
    emit(run(&input))
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

fn run(input_path: &Path) -> Outcome {
    let bytes = if input_path == Path::new("-") {
        let mut bytes = Vec::new();
        if let Err(error) = io::stdin().read_to_end(&mut bytes) {
            return Outcome::failure(format!("Cannot read stdin: {error}"));
        }
        bytes
    } else {
        match fs::read(input_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Outcome::failure(format!(
                    "Cannot read input {}: {error}",
                    input_path.display()
                ));
            }
        }
    };
    let root = match env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            return Outcome::failure(format!("Cannot resolve current directory: {error}"));
        }
    };
    apply_json(&root, &bytes)
}
