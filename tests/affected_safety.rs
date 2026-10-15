use executor::{Outcome, apply_json};
use git2::Repository;
use serde_json::{Value, json};
use std::{fs, path::Path};
use tempfile::{TempDir, tempdir};

fn fixture() -> TempDir {
    let dir = tempdir().unwrap();
    Repository::init(dir.path()).unwrap();
    dir
}
fn execute(root: &Path, operations: Vec<Value>) -> Outcome {
    let input = serde_json::to_vec(&json!({"operations":operations})).unwrap();
    apply_json(root, &input)
}
fn successful_targets(root: &Path, operations: Vec<Value>, targets: &[&str]) {
    let count = operations.len();
    let outcome = execute(root, operations);
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(outcome.operations_applied, count);
    assert_eq!(outcome.affected_files, targets);
}

#[test]
fn empty_batches_directory_operations_and_recreation() {
    let dir = fixture();
    successful_targets(dir.path(), vec![], &[]);
    successful_targets(
        dir.path(),
        vec![
            json!({"type":"create_directory","path":"empty"}),
            json!({"type":"move_directory","from":"empty","to":"gone"}),
            json!({"type":"delete_directory","path":"gone"}),
        ],
        &[],
    );
    successful_targets(
        dir.path(),
        vec![
            json!({"type":"create_directory","path":"dir"}),
            json!({"type":"create_file","path":"dir/a.rs","content":"one"}),
            json!({"type":"delete_file","path":"dir/a.rs"}),
            json!({"type":"delete_directory","path":"dir"}),
            json!({"type":"create_directory","path":"dir"}),
            json!({"type":"create_file","path":"dir/a.rs","content":"two"}),
        ],
        &["dir/a.rs"],
    );
}

#[test]
fn move_with_content_change() {
    let dir = fixture();
    fs::write(dir.path().join("old.rs"), "before\n").unwrap();
    successful_targets(
        dir.path(),
        vec![
            json!({"type":"edit_file","path":"old.rs","edits":[{"oldText":"before","newText":"after"}]}),
            json!({"type":"move_file","from":"old.rs","to":"new.rs"}),
        ],
        &["new.rs"],
    );
    assert!(!dir.path().join("old.rs").exists());
    assert_eq!(fs::read(dir.path().join("new.rs")).unwrap(), b"after\n");
}

#[test]
fn failures_clear_targets_and_preserve_existing_counts() {
    let dir = fixture();
    let outcome = execute(
        dir.path(),
        vec![
            json!({"type":"create_file","path":"kept.rs","content":"kept"}),
            json!({"type":"delete_file","path":"missing.rs"}),
            json!({"type":"create_file","path":"never.rs","content":"never"}),
        ],
    );
    assert!(!outcome.success);
    assert_eq!(outcome.operations_applied, 1);
    assert_eq!(outcome.failed_operation, Some(1));
    assert!(outcome.affected_files.is_empty());
    assert!(dir.path().join("kept.rs").is_file());
    assert!(!dir.path().join("never.rs").exists());
    for outcome in [
        apply_json(dir.path(), b"{"),
        apply_json(&dir.path().join("missing"), br#"{"operations":[]}"#),
        Outcome::failure("CLI error".into()),
    ] {
        assert!(!outcome.success);
        assert_eq!(
            serde_json::to_value(outcome).unwrap()["affected_files"],
            json!([])
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_case_aliases_deduplicate_and_junctions_are_not_followed() {
    use sha2::{Digest, Sha256};
    use std::process::Command;
    let dir = fixture();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("outside.rs"), "outside").unwrap();
    fs::create_dir(dir.path().join("dir")).unwrap();
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(dir.path().join("dir").join("junction"))
        .arg(outside.path())
        .status()
        .unwrap();
    assert!(status.success());
    let hash = format!("{:x}", Sha256::digest(b"same"));
    successful_targets(
        dir.path(),
        vec![
            json!({"type":"create_file","path":"dir/regular.rs","content":"same"}),
            json!({"type":"replace_file","path":"DIR/REGULAR.RS","expected_sha256":hash,"content":"same"}),
        ],
        &["dir/regular.rs"],
    );
    successful_targets(
        dir.path(),
        vec![json!({"type":"move_directory","from":"dir","to":"moved"})],
        &["moved/regular.rs"],
    );
    assert_eq!(
        fs::read_to_string(outside.path().join("outside.rs")).unwrap(),
        "outside"
    );
}

#[cfg(unix)]
#[test]
fn unix_walk_excludes_symlinks_dangling_links_and_sockets() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let dir = fixture();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("outside.rs"), "outside").unwrap();
    fs::create_dir_all(dir.path().join("dir/nested")).unwrap();
    fs::write(dir.path().join("dir/nested/regular.rs"), "regular").unwrap();
    symlink(outside.path(), dir.path().join("dir/external")).unwrap();
    symlink("nested/regular.rs", dir.path().join("dir/link.rs")).unwrap();
    symlink("missing", dir.path().join("dir/dangling")).unwrap();
    let _socket = UnixListener::bind(dir.path().join("dir/socket")).unwrap();
    successful_targets(
        dir.path(),
        vec![json!({"type":"move_directory","from":"dir","to":"moved"})],
        &["moved/nested/regular.rs"],
    );
    assert_eq!(
        fs::read_to_string(outside.path().join("outside.rs")).unwrap(),
        "outside"
    );
}
