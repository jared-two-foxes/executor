use executor::{Input, Operation, Outcome, TextEdit, apply, apply_json};
use git2::Repository;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
use tempfile::{TempDir, tempdir};

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        let dir = tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("file.txt"), bytes).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("file.txt")).unwrap();
        index.write().unwrap();
        Self { dir }
    }

    fn run(&self, operations: Value) -> Outcome {
        let input = json!({"operations": operations});
        apply_json(self.dir.path(), input.to_string().as_bytes())
    }

    fn bytes(&self) -> Vec<u8> {
        fs::read(self.dir.path().join("file.txt")).unwrap()
    }

    fn assert_failure(&self, operations: Value, index: usize, messages: &[&str]) {
        let before = self.bytes();
        let result = self.run(operations);
        assert!(!result.success);
        assert_eq!(result.operations_applied, index);
        assert_eq!(result.failed_operation, Some(index));
        assert!(result.affected_files.is_empty());
        let error = result.error.unwrap();
        assert!(error.contains("edit_file"), "{error}");
        for message in messages {
            assert!(error.contains(message), "{message}: {error}");
        }
        assert_eq!(self.bytes(), before);
    }
}

fn edit(path: &str, old: &str, new: &str) -> Value {
    json!({
        "type": "edit_file", "path": path,
        "edits": [{"oldText": old, "newText": new}]
    })
}

#[test]
fn cli_accepts_exact_text_json_and_preserves_the_index() {
    let f = Fixture::new(b"original\n");
    let index_path = f.dir.path().join(".git/index");
    let before = fs::read(&index_path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_executor"))
        .args(["apply", "-"])
        .current_dir(f.dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = json!({"operations": [edit("file.txt", "original", "updated")]});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result,
        json!({"success": true, "operations_applied": 1, "affected_files": ["file.txt"]})
    );
    assert_eq!(f.bytes(), b"updated\n");
    assert_eq!(fs::read(&index_path).unwrap(), before);
}

#[test]
fn typed_library_api_accepts_public_text_edits() {
    let f = Fixture::new(b"old");
    let input = Input {
        operations: vec![Operation::EditFile {
            path: "file.txt".into(),
            edits: vec![TextEdit {
                old_text: "old".into(),
                new_text: "new".into(),
            }],
        }],
    };
    let result = apply(f.dir.path(), &input);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.affected_files, ["file.txt"]);
    assert_eq!(f.bytes(), b"new");
}

#[test]
fn multiple_edits_resolve_against_one_original_snapshot() {
    for replacements in [
        json!([{"oldText":"left","newText":"right"}, {"oldText":"right","newText":"done"}]),
        json!([{"oldText":"right","newText":"done"}, {"oldText":"left","newText":"right"}]),
    ] {
        let f = Fixture::new(b"left right");
        let result = f.run(json!([{
            "type":"edit_file", "path":"file.txt", "edits":replacements
        }]));
        assert!(result.success, "{:?}", result.error);
        assert_eq!(result.affected_files, ["file.txt"]);
        assert_eq!(f.bytes(), b"right done");
    }
}

#[test]
fn all_match_validation_errors_leave_the_file_unchanged() {
    let f = Fixture::new(b"alpha beta");
    for (replacements, messages) in [
        (
            json!([{"oldText":"alpha","newText":"changed"}, {"oldText":"absent","newText":"x"}]),
            vec!["file.txt", "edit 1", "missing"],
        ),
        (
            json!([{"oldText":"alpha","newText":"changed"}, {"oldText":"a","newText":"x"}]),
            vec!["file.txt", "edit 1", "ambiguous"],
        ),
        (
            json!([{"oldText":"alpha","newText":"x"}, {"oldText":"pha b","newText":"y"}]),
            vec!["file.txt", "edits 0 and 1", "overlapping"],
        ),
        (
            json!([{"oldText":"alpha","newText":"changed"}, {"oldText":"","newText":"x"}]),
            vec!["file.txt", "edit 1", "oldText must not be empty"],
        ),
        (json!([]), vec!["file.txt", "edits must not be empty"]),
        (
            json!([{"oldText":"alpha","newText":"fresh"}, {"oldText":"fresh","newText":"x"}]),
            vec!["file.txt", "edit 1", "missing"],
        ),
    ] {
        f.assert_failure(
            json!([{"type":"edit_file","path":"file.txt","edits":replacements}]),
            0,
            &messages,
        );
    }
    let repeated = Fixture::new(b"aaa");
    repeated.assert_failure(
        json!([edit("file.txt", "aa", "x")]),
        0,
        &["file.txt", "edit 0", "ambiguous"],
    );
    let empty = Fixture::new(b"");
    empty.assert_failure(
        json!([edit("file.txt", "absent", "x")]),
        0,
        &["file.txt", "edit 0", "missing"],
    );
}

#[test]
fn deletion_preserves_unicode_bom_nul_and_mixed_line_endings() {
    let f = Fixture::new("\u{feff}α\r\nremove\r\nkeep\n\0tail\r".as_bytes());
    let result = f.run(json!([{
        "type": "edit_file", "path": "file.txt",
        "edits": [
            {"oldText": "remove\r\n", "newText": ""},
            {"oldText": "α", "newText": "β🙂"}
        ]
    }]));
    assert!(result.success, "{:?}", result.error);
    assert_eq!(f.bytes(), "\u{feff}β🙂\r\nkeep\n\0tail\r".as_bytes());
    let whole = Fixture::new(b"all");
    assert!(whole.run(json!([edit("file.txt", "all", "")])).success);
    assert_eq!(whole.bytes(), b"");
}

#[test]
fn line_endings_are_not_normalized_for_matching() {
    let f = Fixture::new(b"alpha\r\nbeta");
    f.assert_failure(
        json!([edit("file.txt", "alpha\nbeta", "x")]),
        0,
        &["file.txt", "missing"],
    );
}

#[test]
fn invalid_utf8_is_rejected_without_lossy_decoding_or_writes() {
    for bytes in [b"\xffalpha".as_slice(), b"\xff\xfeA\0".as_slice()] {
        let f = Fixture::new(bytes);
        f.assert_failure(
            json!([edit("file.txt", "alpha", "x")]),
            0,
            &["file.txt", "UTF-8"],
        );
    }
}

#[test]
fn paths_must_be_safe_existing_regular_files() {
    let f = Fixture::new(b"alpha");
    let config_path = f.dir.path().join(".git/config");
    let config = fs::read(&config_path).unwrap();
    fs::create_dir(f.dir.path().join("directory")).unwrap();
    for path in [
        "",
        "../file.txt",
        "nested/../../file.txt",
        ".git/config",
        ".GIT/config",
        "git~1/config",
        "file.txt:stream",
        r"nested\file.txt",
        "/file.txt",
        "file.txt.",
        "file.txt ",
        "missing.txt",
        "directory",
    ] {
        f.assert_failure(json!([edit(path, "alpha", "x")]), 0, &[path]);
    }
    assert!(!f.dir.path().join("missing.txt").exists());
    assert!(f.dir.path().join("directory").is_dir());
    assert_eq!(fs::read(&config_path).unwrap(), config);
    let outside = tempdir().unwrap();
    let external = outside.path().join("external.txt");
    fs::write(&external, b"alpha").unwrap();
    f.assert_failure(
        json!([edit(external.to_str().unwrap(), "alpha", "x")]),
        0,
        &[],
    );
    assert_eq!(fs::read(&external).unwrap(), b"alpha");
}

#[test]
fn malformed_edit_shapes_fail_before_any_operations_run() {
    let f = Fixture::new(b"alpha");
    for invalid in [
        json!({"type":"edit_file","path":"file.txt"}),
        json!({"type":"edit_file","path":"file.txt","edits":null}),
        json!({"type":"edit_file","path":"file.txt","edits":[{"oldText":"alpha"}]}),
        json!({"type":"edit_file","path":"file.txt","edits":[{"old_text":"alpha","newText":"x"}]}),
        json!({"type":"edit_file","path":"file.txt","edits":[{"oldText":"alpha","newText":"x","extra":true}]}),
        json!({"type":"edit_file","path":"file.txt","edits":[{"oldText":1,"newText":"x"}]}),
    ] {
        let result = f.run(json!([
            {"type":"create_file","path":"never.txt","content":"never"},
            invalid
        ]));
        assert!(!result.success);
        assert_eq!(result.operations_applied, 0);
        assert_eq!(result.failed_operation, None);
        assert!(result.affected_files.is_empty());
        assert!(!f.dir.path().join("never.txt").exists());
        assert_eq!(f.bytes(), b"alpha");
    }
}

#[test]
fn failure_preserves_prior_success_and_stops_the_batch() {
    let f = Fixture::new(b"alpha");
    f.assert_failure(
        json!([
            {"type":"create_file","path":"kept.txt","content":"kept"},
            edit("file.txt", "missing", "x"),
            {"type":"delete_file","path":"kept.txt"}
        ]),
        1,
        &["file.txt", "edit 0", "missing"],
    );
    assert_eq!(fs::read(f.dir.path().join("kept.txt")).unwrap(), b"kept");
}

#[test]
fn edits_compose_with_creates_moves_and_deletes() {
    let f = Fixture::new(b"untouched");
    let result = f.run(json!([
        {"type":"create_directory","path":"draft"},
        {"type":"create_file","path":"draft/a.txt","content":"one\n"},
        edit("draft/a.txt", "one", "two"),
        edit("draft/a.txt", "two", "three"),
        {"type":"move_file","from":"draft/a.txt","to":"draft/b.txt"},
        edit("draft/b.txt", "three", "four"),
        {"type":"delete_file","path":"draft/b.txt"},
        {"type":"create_file","path":"draft/b.txt","content":"reborn"},
        edit("draft/b.txt", "reborn", "final"),
        {"type":"move_directory","from":"draft","to":"finished"}
    ]));
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.operations_applied, 10);
    assert_eq!(result.affected_files, ["finished/b.txt"]);
    assert_eq!(
        fs::read(f.dir.path().join("finished/b.txt")).unwrap(),
        b"final"
    );
    assert!(!f.dir.path().join("draft").exists());
    assert_eq!(f.bytes(), b"untouched");
}

#[test]
fn identical_edits_are_reported_and_later_deletes_remove_targets() {
    let f = Fixture::new(b"same");
    let result = f.run(json!([edit("file.txt", "same", "same")]));
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.affected_files, ["file.txt"]);
    assert_eq!(f.bytes(), b"same");
    let result = f.run(json!([
        edit("file.txt", "same", "changed"),
        {"type":"move_file","from":"file.txt","to":"moved.txt"},
        {"type":"delete_file","path":"moved.txt"}
    ]));
    assert!(result.success, "{:?}", result.error);
    assert!(result.affected_files.is_empty());
    assert!(!f.dir.path().join("moved.txt").exists());
}

#[cfg(unix)]
#[test]
fn executable_permissions_are_preserved_and_symlinks_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let f = Fixture::new(b"alpha");
    let path = f.dir.path().join("file.txt");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let result = f.run(json!([edit("file.txt", "alpha", "updated")]));
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    symlink(&path, f.dir.path().join("link.txt")).unwrap();
    symlink("absent", f.dir.path().join("dangling")).unwrap();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("external.txt"), b"updated").unwrap();
    symlink(outside.path(), f.dir.path().join("linked")).unwrap();
    for target in ["link.txt", "dangling", "linked/external.txt"] {
        f.assert_failure(
            json!([edit(target, "updated", "bad")]),
            0,
            &[target, "symlink"],
        );
    }
    assert_eq!(
        fs::read(outside.path().join("external.txt")).unwrap(),
        b"updated"
    );
}

#[cfg(unix)]
#[test]
fn non_regular_sockets_are_rejected_before_opening() {
    use std::os::unix::net::UnixListener;

    let f = Fixture::new(b"alpha");
    let path = f.dir.path().join("socket");
    let _listener = UnixListener::bind(&path).unwrap();
    f.assert_failure(
        json!([edit("socket", "alpha", "x")]),
        0,
        &["socket", "Unexpected file type"],
    );
    assert!(path.exists());
}
