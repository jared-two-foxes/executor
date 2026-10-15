use executor::{Input, Operation, Outcome, apply, apply_json};
use git2::Repository;
use tempfile::tempdir;

#[test]
fn public_failures_serialize_empty_targets() {
    let dir = tempdir().unwrap();
    Repository::init(dir.path()).unwrap();
    let input = Input {
        operations: vec![Operation::DeleteFile {
            path: "missing.rs".into(),
        }],
    };
    for outcome in [
        Outcome::failure("CLI error".into()),
        apply_json(dir.path(), b"{"),
        apply_json(&dir.path().join("missing"), br#"{"operations":[]}"#),
        apply(dir.path(), &input),
    ] {
        assert!(!outcome.success);
        assert_eq!(outcome.operations_applied, 0);
        assert!(outcome.affected_files.is_empty());
        let value = serde_json::to_value(outcome).unwrap();
        assert_eq!(value["affected_files"], serde_json::json!([]));
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{ffi::OsString, fs, os::unix::ffi::OsStringExt, path::PathBuf};

    #[test]
    fn non_utf8_survivors_fail_only_at_final_reporting() {
        let dir = tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        let name = PathBuf::from(OsString::from_vec(vec![0xff]));
        let input = Input {
            operations: vec![
                Operation::CreateFile {
                    path: "normal.rs".into(),
                    content: "normal".into(),
                },
                Operation::CreateFile {
                    path: name.clone(),
                    content: "file".into(),
                },
            ],
        };
        let outcome = apply(dir.path(), &input);
        assert!(!outcome.success);
        assert_eq!(outcome.operations_applied, 2);
        assert_eq!(outcome.failed_operation, None);
        assert!(outcome.error.as_ref().unwrap().contains("non-UTF-8"));
        assert!(outcome.affected_files.is_empty());
        assert!(dir.path().join(name).is_file());
        assert!(dir.path().join("normal.rs").is_file());
        let name = PathBuf::from(OsString::from_vec(vec![0xfe]));
        let input = Input {
            operations: vec![
                Operation::CreateFile {
                    path: name.clone(),
                    content: "file".into(),
                },
                Operation::DeleteFile { path: name },
            ],
        };
        let outcome = apply(dir.path(), &input);
        assert!(outcome.success, "{:?}", outcome.error);
        assert!(outcome.affected_files.is_empty());
    }

    #[test]
    fn non_utf8_edit_paths_fail_without_lossy_conversion() {
        use executor::TextEdit;
        let dir = tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        let name = PathBuf::from(OsString::from_vec(vec![0xff]));
        fs::write(dir.path().join(&name), "before").unwrap();
        let input = Input {
            operations: vec![Operation::EditFile {
                path: name.clone(),
                edits: vec![TextEdit {
                    old_text: "before".into(),
                    new_text: "after".into(),
                }],
            }],
        };
        let outcome = apply(dir.path(), &input);
        assert!(!outcome.success);
        assert_eq!(outcome.operations_applied, 1);
        assert_eq!(outcome.failed_operation, None);
        assert!(outcome.affected_files.is_empty());
        assert!(outcome.error.unwrap().contains("non-UTF-8"));
        assert_eq!(fs::read(dir.path().join(name)).unwrap(), b"after");
    }

    #[test]
    fn non_utf8_directory_descendants_use_final_surviving_locations() {
        let dir = tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        fs::create_dir(dir.path().join("dir")).unwrap();
        let name = OsString::from_vec(vec![0xff]);
        fs::write(dir.path().join("dir").join(&name), "file").unwrap();
        let input = Input {
            operations: vec![Operation::MoveDirectory {
                from: "dir".into(),
                to: "moved".into(),
            }],
        };
        let outcome = apply(dir.path(), &input);
        assert!(!outcome.success);
        assert_eq!(outcome.operations_applied, 1);
        assert_eq!(outcome.failed_operation, None);
        assert!(outcome.affected_files.is_empty());
        assert!(dir.path().join("moved").join(&name).is_file());
        let input = Input {
            operations: vec![
                Operation::MoveDirectory {
                    from: "moved".into(),
                    to: "final".into(),
                },
                Operation::DeleteFile {
                    path: PathBuf::from("final").join(name),
                },
            ],
        };
        let outcome = apply(dir.path(), &input);
        assert!(outcome.success, "{:?}", outcome.error);
        assert!(outcome.affected_files.is_empty());
    }

    #[test]
    fn enumeration_errors_fail_the_move_before_renaming() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        let restricted = dir.path().join("dir/restricted");
        fs::create_dir_all(&restricted).unwrap();
        fs::write(restricted.join("file.rs"), "file").unwrap();
        let permissions = fs::metadata(&restricted).unwrap().permissions();
        fs::set_permissions(&restricted, fs::Permissions::from_mode(0o0)).unwrap();
        // Privileged users bypass permissions; unprivileged Linux CI covers this.
        if fs::read_dir(&restricted).is_ok() {
            fs::set_permissions(&restricted, permissions).unwrap();
            return;
        }
        let input = Input {
            operations: vec![
                Operation::CreateFile {
                    path: "kept.rs".into(),
                    content: "kept".into(),
                },
                Operation::MoveDirectory {
                    from: "dir".into(),
                    to: "moved".into(),
                },
            ],
        };
        let outcome = apply(dir.path(), &input);
        fs::set_permissions(&restricted, permissions).unwrap();
        assert!(!outcome.success);
        assert_eq!(outcome.operations_applied, 1);
        assert_eq!(outcome.failed_operation, Some(1));
        assert!(outcome.affected_files.is_empty());
        assert!(
            outcome
                .error
                .unwrap()
                .contains("Cannot enumerate affected directory")
        );
        assert!(dir.path().join("dir/restricted/file.rs").is_file());
        assert!(dir.path().join("kept.rs").is_file());
        assert!(!dir.path().join("moved").exists());
    }
}
