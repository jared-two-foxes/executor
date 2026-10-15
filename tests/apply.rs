use git2::{Repository, Signature};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    repo: Repository,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("example.txt"), "original\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("example.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let signature = Signature::now("Test", "test@example.com").unwrap();
        {
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
                .unwrap();
        }
        Self { dir, repo }
    }
    fn apply(&self, operations: &[Value]) -> Value {
        self.raw(
            &json!({ "operations": operations }).to_string(),
            self.dir.path(),
        )
    }
    fn raw(&self, input: &str, cwd: &Path) -> Value {
        // Keep the input outside the target repository.
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("input.json");
        fs::write(&path, input).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_executor"))
            .args(["apply"])
            .arg(path)
            .current_dir(cwd)
            .output()
            .unwrap();
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            output.status.success(),
            result["success"].as_bool().unwrap()
        );
        assert!(output.stderr.is_empty(), "{:?}", output);
        assert!(result["affected_files"].is_array(), "{result}");
        if result["success"] == false {
            assert_eq!(result["affected_files"], json!([]));
        }
        result
    }
    fn stream(&self, input: &str) -> Value {
        let mut child = Command::new(env!("CARGO_BIN_EXE_executor"))
            .args(["apply", "-"])
            .current_dir(self.dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            output.status.success(),
            result["success"].as_bool().unwrap()
        );
        assert!(result["affected_files"].is_array(), "{result}");
        if result["success"] == false {
            assert_eq!(result["affected_files"], json!([]));
        }
        result
    }
    fn content(&self, path: &str) -> String {
        fs::read_to_string(self.dir.path().join(path))
            .unwrap()
            .replace("\r\n", "\n")
    }
}
fn change(path: &str, before: &str, after: &str) -> Value {
    json!({"type":"edit_file","path":path,"edits":[{"oldText":before,"newText":after}]})
}
fn add(path: &str, text: &str) -> Value {
    json!({"type":"create_file","path":path,"content":format!("{text}\n")})
}
#[test]
fn successful_application_only_changes_workdir() {
    let f = Fixture::new();
    let head = f.repo.head().unwrap().target();
    let index = fs::read(f.repo.path().join("index")).unwrap();
    assert_eq!(
        f.apply(&[change("example.txt", "original", "updated")]),
        json!({"success":true,"operations_applied":1,"affected_files":["example.txt"]})
    );
    assert_eq!(f.content("example.txt"), "updated\n");
    assert_eq!(fs::read(f.repo.path().join("index")).unwrap(), index);
    assert_eq!(f.repo.head().unwrap().target(), head);
}
#[test]
fn multiple_operations_and_sequential_dependencies() {
    let f = Fixture::new();
    let result = f.apply(&[
        json!({"type":"create_directory","path":"nested"}),
        add("nested/new.txt", "first"),
        change("nested/new.txt", "first", "second"),
        change("example.txt", "original", "updated"),
    ]);
    assert_eq!(
        result,
        json!({"success":true,"operations_applied":4,"affected_files":["example.txt","nested/new.txt"]})
    );
    assert_eq!(f.content("nested/new.txt"), "second\n");
}
#[test]
fn allows_dirty_tracked_and_untracked_files() {
    let f = Fixture::new();
    fs::write(f.dir.path().join("example.txt"), "dirty\n").unwrap();
    fs::write(f.dir.path().join("untracked.txt"), "existing\n").unwrap();
    let result = f.apply(&[
        change("example.txt", "dirty", "updated"),
        change("untracked.txt", "existing", "changed"),
    ]);
    assert_eq!(result["success"], true);
    assert_eq!(
        result["affected_files"],
        json!(["example.txt", "untracked.txt"])
    );
    assert_eq!(f.content("untracked.txt"), "changed\n");
}
#[test]
fn leaves_unrelated_dirty_and_staged_changes_alone() {
    let f = Fixture::new();
    fs::write(f.dir.path().join("staged.txt"), "staged\n").unwrap();
    let mut index = f.repo.index().unwrap();
    index.add_path(Path::new("staged.txt")).unwrap();
    index.write().unwrap();
    fs::write(f.dir.path().join("staged.txt"), "dirty\n").unwrap();
    fs::write(f.dir.path().join("unrelated.txt"), "untracked").unwrap();
    let bytes = fs::read(f.repo.path().join("index")).unwrap();
    let result = f.apply(&[change("example.txt", "original", "updated")]);
    assert_eq!(result["success"], true);
    assert_eq!(result["affected_files"], json!(["example.txt"]));
    assert_eq!(f.content("unrelated.txt"), "untracked");
    assert_eq!(f.content("staged.txt"), "dirty\n");
    assert_eq!(fs::read(f.repo.path().join("index")).unwrap(), bytes);
}
#[test]
fn conflicting_edit_stops_after_partial_success() {
    let f = Fixture::new();
    let result = f.apply(&[
        change("example.txt", "original", "updated"),
        change("example.txt", "original", "conflict"),
        add("never.txt", "never"),
    ]);
    assert_eq!(result["success"], false);
    assert_eq!(result["operations_applied"], 1);
    assert_eq!(result["failed_operation"], 1);
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("missing")
    );
    assert_eq!(f.content("example.txt"), "updated\n");
    assert!(!f.dir.path().join("never.txt").exists());
}
#[test]
fn missing_edit_match_reports_zero() {
    let f = Fixture::new();
    let result = f.apply(&[change("example.txt", "wrong", "updated")]);
    assert_eq!(result["operations_applied"], 0);
    assert_eq!(result["failed_operation"], 0);
    assert_eq!(f.content("example.txt"), "original\n");
}
#[test]
fn invalid_input_is_rejected_before_changes() {
    let f = Fixture::new();
    for input in [
        "{",
        "{}",
        "[]",
        r#"{"patches":[1]}"#,
        r#"{"patches":[],"repository":"elsewhere"}"#,
        r#"{"operations":[],"repository":"elsewhere"}"#,
    ] {
        let result = f.raw(input, f.dir.path());
        assert_eq!(result["success"], false);
        assert_eq!(result["operations_applied"], 0);
        assert!(result.get("failed_operation").is_none());
    }
}
#[test]
fn invalid_edit_after_success_preserves_changes() {
    let f = Fixture::new();
    let result = f.apply(&[
        add("kept.txt", "kept"),
        json!({"type":"edit_file","path":"example.txt","edits":[]}),
    ]);
    assert_eq!(result["failed_operation"], 1);
    assert_eq!(result["operations_applied"], 1);
    assert_eq!(f.content("kept.txt"), "kept\n");
    assert_eq!(f.content("example.txt"), "original\n");
}
#[test]
fn empty_sequence_succeeds() {
    let f = Fixture::new();
    assert_eq!(
        f.apply(&[]),
        json!({"success":true,"operations_applied":0,"affected_files":[]})
    );
}
#[test]
fn rejects_traversal_absolute_windows_and_git_metadata_paths() {
    let f = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    let absolute = outside
        .path()
        .join("escape.txt")
        .to_string_lossy()
        .into_owned();
    for path in [
        "../escape.txt",
        "nested/../../escape.txt",
        ".git/config",
        ".GIT/config",
        ".git./config",
        "C:/escape.txt",
        "nested\\escape.txt",
        absolute.as_str(),
    ] {
        let result = f.apply(&[add(path, "escape")]);
        assert_eq!(result["success"], false, "accepted {path}: {result}");
        assert_eq!(result["failed_operation"], 0);
    }
    assert!(!outside.path().join("escape.txt").exists());
}
#[test]
fn requires_repository_root() {
    let f = Fixture::new();
    let nested = f.dir.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let outside = tempfile::tempdir().unwrap();
    for cwd in [nested.as_path(), outside.path()] {
        assert_eq!(f.raw(r#"{"operations":[]}"#, cwd)["success"], false);
    }
    let bare = tempfile::tempdir().unwrap();
    Repository::init_bare(bare.path()).unwrap();
    assert_eq!(f.raw(r#"{"operations":[]}"#, bare.path())["success"], false);
}
#[test]
fn missing_input_and_cli_errors_are_json() {
    let f = Fixture::new();
    for args in [
        vec!["apply", "missing.json"],
        vec!["apply"],
        vec!["unknown"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_executor"))
            .args(args)
            .current_dir(f.dir.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["success"], false);
        assert_eq!(result["operations_applied"], 0);
        assert_eq!(result["affected_files"], json!([]));
    }
}
#[cfg(unix)]
#[test]
fn rejects_symlink_ancestors_final_paths_and_dangling_links() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("victim.txt"), "original\n").unwrap();
    symlink(outside.path(), f.dir.path().join("linked")).unwrap();
    symlink(
        outside.path().join("victim.txt"),
        f.dir.path().join("victim.txt"),
    )
    .unwrap();
    symlink(
        outside.path().join("absent.txt"),
        f.dir.path().join("dangling.txt"),
    )
    .unwrap();
    for operation in [
        change("linked/victim.txt", "original", "bad"),
        change("victim.txt", "original", "bad"),
        add("linked/new.txt", "bad"),
        add("dangling.txt", "bad"),
    ] {
        assert_eq!(f.apply(&[operation])["success"], false);
    }
    assert_eq!(
        fs::read_to_string(outside.path().join("victim.txt")).unwrap(),
        "original\n"
    );
    assert!(!outside.path().join("new.txt").exists());
    assert!(!outside.path().join("absent.txt").exists());
}

#[cfg(windows)]
#[test]
fn rejects_junction_ancestors() {
    let f = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("victim.txt"), "original\n").unwrap();
    let junction = f.dir.path().join("linked");
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(outside.path())
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(
        f.apply(&[change("linked/victim.txt", "original", "bad")])["success"],
        false
    );
    assert_eq!(
        fs::read_to_string(outside.path().join("victim.txt")).unwrap(),
        "original\n"
    );
}
#[test]
fn unsafe_later_operation_preserves_earlier_success() {
    let f = Fixture::new();
    let result = f.apply(&[add("kept.txt", "kept"), add("../outside.txt", "bad")]);
    assert_eq!(result["operations_applied"], 1);
    assert_eq!(result["failed_operation"], 1);
    assert_eq!(f.content("kept.txt"), "kept\n");
}
#[test]
fn deletes_a_file() {
    let f = Fixture::new();
    assert_eq!(
        f.apply(&[json!({"type":"delete_file","path":"example.txt"})])["success"],
        true
    );
    assert!(!f.dir.path().join("example.txt").exists());
}

#[test]
fn unsafe_move_sources_and_destinations_are_rejected() {
    let f = Fixture::new();
    for operation in [
        json!({"type":"move_file","from":"../escape.txt","to":"safe.txt"}),
        json!({"type":"move_file","from":"example.txt","to":"../escape.txt"}),
        json!({"type":"move_file","from":".git/config","to":"safe.txt"}),
        json!({"type":"move_file","from":"example.txt","to":".git/config"}),
    ] {
        assert_eq!(f.apply(&[operation])["failed_operation"], 0);
    }
    assert!(!f.dir.path().join("safe.txt").exists());
    assert_eq!(f.content("example.txt"), "original\n");
}


#[test]
fn existing_untracked_file_is_not_overwritten_by_addition() {
    let f = Fixture::new();
    fs::write(f.dir.path().join("untracked.txt"), "existing\n").unwrap();
    let result = f.apply(&[add("untracked.txt", "overwrite")]);
    assert_eq!(result["success"], false);
    assert_eq!(f.content("untracked.txt"), "existing\n");
}

#[test]
fn json_file_input_resolves_operation_paths_relative_to_cwd() {
    let f = Fixture::new();
    let input = json!({"operations":[
        change("example.txt", "original", "first"),
        change("example.txt", "first", "second"),
        change("example.txt", "second", "third")
    ]});
    // raw() stores the JSON input outside the target repository.
    assert_eq!(
        f.raw(&input.to_string(), f.dir.path()),
        json!({"success":true,"operations_applied":3,"affected_files":["example.txt"]})
    );
    assert_eq!(f.content("example.txt"), "third\n");
}

#[test]
fn stdin_accepts_ordered_operations_and_reports_json() {
    let f = Fixture::new();
    let input = json!({"operations":[
        change("example.txt", "original", "first"),
        change("example.txt", "first", "second")
    ]});
    assert_eq!(
        f.stream(&input.to_string()),
        json!({"success":true,"operations_applied":2,"affected_files":["example.txt"]})
    );
    assert_eq!(f.content("example.txt"), "second\n");
}

#[test]
fn missing_file_after_success_reports_its_index() {
    let f = Fixture::new();
    let input = json!({"operations":[
        change("example.txt", "original", "first"),
        {"type":"delete_file","path":"missing.txt"},
        change("example.txt", "first", "never")
    ]});
    let result = f.stream(&input.to_string());
    assert_eq!(result["success"], false);
    assert_eq!(result["operations_applied"], 1);
    assert_eq!(result["failed_operation"], 1);
    assert!(result["error"].as_str().unwrap().contains("missing.txt"));
    assert_eq!(f.content("example.txt"), "first\n");
}

#[test]
fn removed_patch_types_fail_before_any_cli_operations_execute() {
    let f = Fixture::new();
    let head = f.repo.head().unwrap().target();
    let index = fs::read(f.repo.path().join("index")).unwrap();
    for source in [
        json!({"type":"inline","patch":"diff --git a/a b/a\n"}),
        json!({"type":"file","path":"missing.patch"}),
    ] {
        let input = json!({"operations":[
            add("never.txt", "never"),
            {"type":"patch","source":source},
            {"type":"delete_file","path":"example.txt"}
        ]});
        for result in [
            f.stream(&input.to_string()),
            f.raw(&input.to_string(), f.dir.path()),
        ] {
            assert_eq!(result["success"], false);
            assert_eq!(result["operations_applied"], 0);
            assert_eq!(result["affected_files"], json!([]));
            assert!(result.get("failed_operation").is_none());
            assert!(result["error"].as_str().unwrap().contains("unknown variant `patch`"));
            assert!(!f.dir.path().join("never.txt").exists());
            assert_eq!(f.content("example.txt"), "original\n");
        }
    }
    assert_eq!(fs::read(f.repo.path().join("index")).unwrap(), index);
    assert_eq!(f.repo.head().unwrap().target(), head);
    assert_eq!(f.stream("{")["success"], false);
}

#[test]
fn operations_create_move_and_delete_files_and_directories() {
    let f = Fixture::new();
    let input = json!({"operations": [
        {"type":"create_directory", "path":"draft"},
        {"type":"create_file", "path":"draft/note.txt", "content":"hello\n"},
        {"type":"move_file", "from":"draft/note.txt", "to":"draft/renamed.txt"},
        {"type":"move_directory", "from":"draft", "to":"published"},
        {"type":"delete_file", "path":"published/renamed.txt"},
        {"type":"delete_directory", "path":"published"}
    ]});
    assert_eq!(
        f.stream(&input.to_string()),
        json!({"success":true,"operations_applied":6,"affected_files":[]})
    );
    assert!(!f.dir.path().join("draft").exists());
    assert!(!f.dir.path().join("published").exists());
}

#[test]
fn ordered_edits_and_moves_use_current_contents() {
    let f = Fixture::new();
    let input = json!({"operations":[
        change("example.txt", "original", "first"),
        change("example.txt", "first", "second"),
        {"type":"move_file","from":"example.txt","to":"moved.txt"}
    ]});
    assert_eq!(
        f.stream(&input.to_string()),
        json!({"success":true,"operations_applied":3,"affected_files":["moved.txt"]})
    );
    assert_eq!(f.content("moved.txt"), "second\n");
    assert!(!f.dir.path().join("example.txt").exists());
}

#[test]
fn operations_stop_on_failure_and_preserve_previous_changes() {
    let f = Fixture::new();
    let input = json!({"operations": [
        {"type":"create_file", "path":"kept.txt", "content":"kept"},
        {"type":"move_file", "from":"kept.txt", "to":"example.txt"},
        {"type":"delete_file", "path":"kept.txt"}
    ]});
    let result = f.stream(&input.to_string());
    assert_eq!(result["success"], false);
    assert_eq!(result["operations_applied"], 1);
    assert_eq!(result["failed_operation"], 1);
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("Destination already exists")
    );
    assert_eq!(f.content("kept.txt"), "kept");
    assert_eq!(f.content("example.txt"), "original\n");
}

#[test]
fn deleting_a_directory_requires_it_to_be_empty() {
    let f = Fixture::new();
    let input = json!({"operations": [
        {"type":"create_directory", "path":"occupied"},
        {"type":"create_file", "path":"occupied/file.txt", "content":"keep"},
        {"type":"delete_directory", "path":"occupied"}
    ]});
    let result = f.stream(&input.to_string());
    assert_eq!(result["operations_applied"], 2);
    assert_eq!(result["failed_operation"], 2);
    assert_eq!(f.content("occupied/file.txt"), "keep");
}

#[test]
fn operation_paths_cannot_escape_or_touch_git_metadata() {
    let f = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("outside.txt");
    for path in [
        "../outside.txt",
        ".git/config",
        ".GIT/config",
        "nested/../../outside.txt",
        "nested\\outside.txt",
    ] {
        let input = json!({"operations":[{"type":"create_file","path":path,"content":"bad"}]});
        let result = f.stream(&input.to_string());
        assert_eq!(result["failed_operation"], 0, "{path}: {result}");
    }
    let input = json!({"operations":[{"type":"create_file","path":external,"content":"bad"}]});
    assert_eq!(f.stream(&input.to_string())["failed_operation"], 0);
    assert!(!external.exists());
    let input =
        json!({"operations":[{"type":"move_file","from":"example.txt","to":"../outside.txt"}]});
    assert_eq!(f.stream(&input.to_string())["failed_operation"], 0);
    assert_eq!(f.content("example.txt"), "original\n");
}

#[cfg(unix)]
#[test]
fn operation_paths_reject_existing_symlink_ancestors() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), f.dir.path().join("linked")).unwrap();
    let input =
        json!({"operations":[{"type":"create_file","path":"linked/escape.txt","content":"bad"}]});
    assert_eq!(f.stream(&input.to_string())["failed_operation"], 0);
    assert!(!outside.path().join("escape.txt").exists());
}

#[test]
fn invalid_operations_input_fails_before_work() {
    let f = Fixture::new();
    for input in [
        json!({"operations":[{"type":"create_file","path":"new.txt"}]}),
        json!({"operations":[{"type":"unknown","path":"new.txt"}]}),
        json!({"operations":[],"patches":[]}),
        json!({"operations":[{"type":"delete_file","path":"example.txt","extra":true}]}),
    ] {
        let result = f.stream(&input.to_string());
        assert_eq!(result["success"], false);
        assert_eq!(result["operations_applied"], 0);
        assert!(result.get("failed_operation").is_none());
    }
    assert_eq!(f.content("example.txt"), "original\n");
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn replace_file_requires_matching_hash_and_preserves_index() {
    let f = Fixture::new();
    let index = fs::read(f.repo.path().join("index")).unwrap();
    let head = f.repo.head().unwrap().target();
    let input = json!({"operations":[{
        "type":"replace_file", "path":"example.txt",
        "expected_sha256":sha256(b"original\n"), "content":"replacement\n"
    }]});
    assert_eq!(
        f.stream(&input.to_string()),
        json!({"success":true,"operations_applied":1,"affected_files":["example.txt"]})
    );
    assert_eq!(f.content("example.txt"), "replacement\n");
    assert_eq!(fs::read(f.repo.path().join("index")).unwrap(), index);
    assert_eq!(f.repo.head().unwrap().target(), head);
}

#[test]
fn replace_file_detects_intervening_edits_without_changing_them() {
    let f = Fixture::new();
    fs::write(f.dir.path().join("example.txt"), "dirty\n").unwrap();
    let input = json!({"operations":[{
        "type":"replace_file", "path":"example.txt",
        "expected_sha256":sha256(b"original\n"), "content":"replacement\n"
    }]});
    let result = f.stream(&input.to_string());
    assert_eq!(result["operations_applied"], 0);
    assert_eq!(result["failed_operation"], 0);
    assert!(result["error"].as_str().unwrap().contains("hash mismatch"));
    assert_eq!(f.content("example.txt"), "dirty\n");
}

#[test]
fn replace_file_uses_state_after_prior_operations() {
    let f = Fixture::new();
    let input = json!({"operations":[
        {"type":"create_file", "path":"new.txt", "content":"first\n"},
        {"type":"replace_file", "path":"new.txt", "expected_sha256":sha256(b"first\n"), "content":"second\n"}
    ]});
    assert_eq!(
        f.stream(&input.to_string()),
        json!({"success":true,"operations_applied":2,"affected_files":["new.txt"]})
    );
    assert_eq!(f.content("new.txt"), "second\n");
}

#[test]
fn replace_file_rejects_invalid_hash_and_unsafe_target() {
    let f = Fixture::new();
    let invalid_hashes = [String::new(), "abcd".into(), "Z".repeat(64), "A".repeat(64)];
    for hash in invalid_hashes {
        let input = json!({"operations":[{
            "type":"replace_file", "path":"example.txt",
            "expected_sha256":hash, "content":"bad"
        }]});
        assert_eq!(f.stream(&input.to_string())["failed_operation"], 0);
    }
    for path in ["../example.txt", ".git/config", "missing.txt"] {
        let input = json!({"operations":[{
            "type":"replace_file", "path":path,
            "expected_sha256":sha256(b"original\n"), "content":"bad"
        }]});
        assert_eq!(f.stream(&input.to_string())["failed_operation"], 0);
    }
    assert_eq!(f.content("example.txt"), "original\n");
}

#[cfg(unix)]
#[test]
fn replace_file_preserves_executable_permission_and_rejects_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let path = f.dir.path().join("example.txt");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let input = json!({"operations":[{
        "type":"replace_file", "path":"example.txt",
        "expected_sha256":sha256(b"original\n"), "content":"updated\n"
    }]});
    assert_eq!(f.stream(&input.to_string())["success"], true);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    symlink(&path, f.dir.path().join("link.txt")).unwrap();
    let input = json!({"operations":[{
        "type":"replace_file", "path":"link.txt",
        "expected_sha256":sha256(b"updated\n"), "content":"bad"
    }]});
    assert_eq!(f.stream(&input.to_string())["failed_operation"], 0);
    assert_eq!(f.content("example.txt"), "updated\n");
}

#[test]
fn accepts_unicode_and_json_escaped_paths() {
    let f = Fixture::new();
    let input = r#"{"operations":[{"type":"create_file","path":"\u00e9.txt","content":"created\n"}]}"#;
    assert_eq!(f.stream(input)["affected_files"], json!(["é.txt"]));
    assert_eq!(f.content("é.txt"), "created\n");
    assert_eq!(
        f.apply(&[
            json!({"type":"create_directory","path":"nested"}),
            add("nested/é.txt", "created"),
        ])["affected_files"],
        json!(["nested/é.txt"])
    );
    #[cfg(unix)]
    {
        assert_eq!(f.apply(&[add("tab\tquote\".txt", "escaped")])["success"], true);
        assert_eq!(f.content("tab\tquote\".txt"), "escaped\n");
    }
}

#[test]
fn rejects_unsafe_json_escaped_paths() {
    let f = Fixture::new();
    for input in [
        r#"{"operations":[{"type":"create_file","path":"nested\u005cescape.txt","content":"bad"}]}"#,
        r#"{"operations":[{"type":"create_file","path":"\u002e\u002e/escape.txt","content":"bad"}]}"#,
        r#"{"operations":[{"type":"create_file","path":"\u002egit/config","content":"bad"}]}"#,
        r#"{"operations":[{"type":"create_file","path":"C\u003a/escape.txt","content":"bad"}]}"#,
    ] {
        assert_eq!(f.stream(input)["failed_operation"], 0, "{input}");
    }
    assert!(!f.dir.path().join("nested").exists());
}

#[test]
fn invalid_unicode_json_paths_fail_before_work() {
    let f = Fixture::new();
    let input = r#"{"operations":[{"type":"create_file","path":"never.txt","content":"never"},{"type":"create_file","path":"\udcff.txt","content":"bad"}]}"#;
    let result = f.stream(input);
    assert_eq!(result["success"], false);
    assert_eq!(result["operations_applied"], 0);
    assert!(result.get("failed_operation").is_none());
    assert!(!f.dir.path().join("never.txt").exists());
}

#[test]
fn help_describes_explicit_operations_and_version_remains_text() {
    let output = Command::new(env!("CARGO_BIN_EXE_executor"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("file and directory operations"));
    assert!(!text.to_lowercase().contains("patch"));
    let output = Command::new(env!("CARGO_BIN_EXE_executor"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().starts_with("executor "));
}
