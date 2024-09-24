use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use clap::Parser;
use std::process::Command;
use walkdir::WalkDir;

#[allow(dead_code)]
#[derive(Debug)]
enum Errors {
    NotReallyAnError(String),
    ActuallyAnError(String),
}

#[derive(Parser, Debug)]
#[clap(version)]
struct Cli {
    /// Path to search for virtualenvs
    path: Option<PathBuf>,
    /// Delete the virtualenvs instead of just printing them
    #[clap(long, short)]
    delete: bool,
    /// Maximum depth to recurse into the directory
    #[clap(long, short)]
    max_depth: Option<usize>,

    /// Go deep - if we find a pyproject.toml, we won't go deeper into a dir structure
    #[clap(long, short = 'D')]
    deep: bool,

    /// Debug mode
    #[clap(long = "debug")]
    debug: bool,

    /// Non-interactive
    #[clap(long = "non-interactive", short)]
    non_interactive: bool,
}

/// gets the size on disk of a directory
fn get_size_on_disk(path: &PathBuf) -> u64 {
    let mut size = 0;
    for entry in WalkDir::new(path) {
        let entry = match entry {
            Ok(val) => val,
            Err(_err) => {
                // eprintln!("Error getting direntry, did you just delete the parent? {:?}", err);
                continue;
            }
        };
        if entry.path().is_file() {
            size += entry.metadata().unwrap().len();
        }
    }
    size
}

/// looks for a virtualenv
fn check_path(
    checked_paths: &mut Vec<PathBuf>,
    cli: &Cli,
    entry: walkdir::DirEntry,
) -> Result<PathBuf, Errors> {
    if !cli.deep {
        for checked_path in checked_paths.iter() {
            if entry.path().starts_with(checked_path) {
                return Err(Errors::NotReallyAnError(format!(
                    "Already checked parent of {}",
                    entry.path().display()
                )));
            }
        }
    }
    if entry.file_name() == "pyproject.toml" {
        checked_paths.push(
            entry
                .path()
                .parent()
                .expect("Can't get parent of a known file?")
                .to_path_buf(),
        );
        let project_path = entry
            .path()
            .parent()
            .expect("Can't find the parent path for a file we just found?");
        if cli.debug {
            eprintln!("Project path: {:?}", project_path);
        }
        let venv = project_path.join(".venv");
        if venv.exists() {
            if cli.debug {
                eprintln!("venv path found: {:?}", venv);
            }
            Ok(venv)
        } else if which::which("poetry").is_ok() {
            // try to use poetry
            if cli.debug {
                eprintln!("venv path not found, trying to run poetry");
            }

            let output = match Command::new("poetry")
                .args([
                    "env",
                    "info",
                    "--path",
                    "--directory",
                    &project_path.display().to_string(),
                ])
                .output()
            {
                Ok(val) => val,
                Err(e) => {
                    return Err(Errors::NotReallyAnError(format!(
                        "Failed to execute poetry command: {:?}",
                        e
                    )));
                }
            };

            if output.status.success() {
                let venv_path = String::from_utf8_lossy(&output.stdout);
                if cli.debug {
                    eprintln!("Virtualenv path from poetry: {:?}", venv_path.trim());
                }
                Ok(PathBuf::from(venv_path.trim()))
            } else {
                Err(Errors::NotReallyAnError(format!(
                    "Failed to get venv path from poetry: {:?}",
                    output.stderr
                )))
            }
        } else {
            Err(Errors::NotReallyAnError(
                "Don't have any other way to ".to_string(),
            ))
        }
    } else {
        Err(Errors::NotReallyAnError("Not pyproject.toml".to_string()))
    }
}

fn main() {
    let cli = Cli::parse();
    let path = &cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
    if cli.debug {
        eprintln!("Walking path: {:?}", path);
    }
    let mut walker = WalkDir::new(path);

    let mut checked_paths = vec![];

    let total_deleted = Arc::new(RwLock::new(0));
    let total_deleted_callback = total_deleted.clone();
    ctrlc::set_handler(move || {
        eprintln!("Received Ctrl+C, exiting...");
        if cli.delete {
            let human_readable_size = byte_unit::Byte::from_u64(
                total_deleted_callback
                    .read()
                    .expect("Failed to get total deleted")
                    .to_owned(),
            )
            .get_appropriate_unit(byte_unit::UnitType::Decimal)
            .to_string();
            eprintln!("Deleted {} of virtualenvs", human_readable_size);
            std::process::exit(0);
        }
    })
    .expect("Error setting Ctrl-C handler");

    if let Some(max_depth) = &cli.max_depth {
        walker = walker.max_depth(*max_depth);
    }

    for entry in walker {
        let entry = match entry {
            Ok(val) => val,
            Err(err) => {
                if cli.debug {
                    eprintln!(
                        "Error getting direntry, did you just delete the parent? {:?}",
                        err
                    );
                }
                continue;
            }
        };
        if !entry.path().exists() {
            if cli.debug {
                eprintln!("Path doesn't exist: {:?}", entry.path());
            }
            continue;
        }

        match check_path(&mut checked_paths, &cli, entry) {
            Err(err) => {
                if let Errors::ActuallyAnError(err) = err {
                    eprintln!("Error: {:?}", err);
                } else if cli.debug {
                    eprintln!("{:?}", err);
                }
            }
            Ok(val) => {
                let dir_size = get_size_on_disk(&val);
                // turn dir_size into a human readable string
                let human_readable_size = byte_unit::Byte::from_u64(dir_size)
                    .get_appropriate_unit(byte_unit::UnitType::Decimal)
                    .to_string();
                if cli.delete {
                    let doit = match cli.non_interactive {
                        true => true,
                        false => {
                            let res = dialoguer::Confirm::new()
                                .with_prompt(format!(
                                    "Delete this? {} ({})",
                                    val.display(),
                                    human_readable_size
                                ))
                                .interact();
                            match res {
                                Ok(val) => val,
                                Err(err) => {
                                    eprintln!("Error getting response from user: {:?}", err);
                                    return;
                                }
                            }
                        }
                    };

                    if doit {
                        if cli.debug {
                            eprintln!("Deleting {}", val.display());
                        }
                        std::fs::remove_dir_all(&val).expect("Failed to delete venv");
                        println!("Deleted {:?} ({})", val.display(), human_readable_size);
                        let mut writer = total_deleted.write().expect("Failed to get write lock");

                        *writer += dir_size;
                    }
                } else {
                    let mut writer = total_deleted.write().expect("Failed to get write lock");
                    *writer += dir_size;
                    println!("Found {:?} ({})", val, human_readable_size);
                }
            }
        };
    }
    let human_readable_size =
        byte_unit::Byte::from_u64(*total_deleted.read().expect("Failed to get reader"))
            .get_appropriate_unit(byte_unit::UnitType::Decimal)
            .to_string();
    if cli.delete {
        eprintln!("Deleted {} of virtualenvs", human_readable_size);
    } else {
        eprintln!("Found {} of virtualenvs", human_readable_size);
    }
}
