use executor::{Input, Operation, PatchSource, apply, apply_json};
use git2::Repository;
use serde_json::json;
use std::{fs, path::PathBuf};
use tempfile::tempdir;

#[test]
fn library_applies_typed_operations_without_a_child_process() {
    let dir = tempdir().unwrap();
    Repository::init(dir.path()).unwrap();
    let input = Input {
        operations: vec![
            Operation::CreateFile {
                path: PathBuf::from("first.txt"),
                content: "created\n".into(),
            },
            Operation::MoveFile {
                from: PathBuf::from("first.txt"),
                to: PathBuf::from("second.txt"),
            },
        ],
    };
    let outcome = apply(dir.path(), &input);
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(outcome.operations_applied, 2);
    assert_eq!(
        fs::read_to_string(dir.path().join("second.txt")).unwrap(),
        "created\n"
    );
}

#[test]
fn library_reports_partial_failure_without_rolling_back() {
    let dir = tempdir().unwrap();
    Repository::init(dir.path()).unwrap();
    let input = json!({
        "operations": [
            {"type":"create_file","path":"created.txt","content":"first"},
            {"type":"delete_file","path":"missing.txt"}
        ]
    });
    let outcome = apply_json(dir.path(), input.to_string().as_bytes());
    assert!(!outcome.success);
    assert_eq!(outcome.operations_applied, 1);
    assert_eq!(outcome.failed_operation, Some(1));
    assert_eq!(
        fs::read_to_string(dir.path().join("created.txt")).unwrap(),
        "first"
    );
}

#[test]
fn library_resolves_patch_file_sources_relative_to_repository_root() {
    let dir = tempdir().unwrap();
    Repository::init(dir.path()).unwrap();
    let patch = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n";
    fs::write(dir.path().join("change.patch"), patch).unwrap();
    let input = Input {
        operations: vec![Operation::Patch {
            source: PatchSource::File {
                path: PathBuf::from("change.patch"),
            },
        }],
    };
    let outcome = apply(dir.path(), &input);
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(
        fs::read_to_string(dir.path().join("new.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "hello\n"
    );
}

#[test]
fn library_rejects_invalid_json_and_non_root_directories() {
    let dir = tempdir().unwrap();
    Repository::init(dir.path()).unwrap();
    let invalid = apply_json(dir.path(), br#"{"operations":[{"type":"unknown"}]}"#);
    assert!(!invalid.success);
    assert_eq!(invalid.operations_applied, 0);
    assert_eq!(invalid.failed_operation, None);
    assert!(invalid.error.unwrap().contains("Invalid input JSON"));

    fs::create_dir(dir.path().join("nested")).unwrap();
    let outcome = apply_json(dir.path().join("nested").as_path(), br#"{"operations":[]}"#);
    assert!(!outcome.success);
    assert_eq!(outcome.operations_applied, 0);
    assert!(outcome.error.unwrap().contains("Git repository root"));
}
