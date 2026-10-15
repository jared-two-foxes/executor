use executor::{Input, Operation, Outcome, apply, apply_json};
use git2::Repository;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
use tempfile::{TempDir, tempdir};

fn fixture() -> TempDir {
    let dir = tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let mut config = repo.config().unwrap();
    config.set_bool("core.autocrlf", false).unwrap();
    config.set_str("core.eol", "lf").unwrap();
    dir
}
fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn execute(root: &Path, operations: Vec<Value>) -> Outcome {
    let input = serde_json::to_vec(&json!({"operations":operations})).unwrap();
    apply_json(root, &input)
}
fn succeeds(root: &Path, operations: Vec<Value>, expected: &[&str]) {
    let count = operations.len();
    let outcome = execute(root, operations);
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(outcome.operations_applied, count);
    let expected: Vec<String> = expected.iter().map(|path| path.to_string()).collect();
    assert_eq!(outcome.affected_files, expected);
    assert_eq!(
        serde_json::to_value(outcome).unwrap(),
        json!({
            "success":true,"operations_applied":count,"affected_files":expected
        })
    );
}
fn create(path: &str, content: &str) -> Value {
    json!({"type":"create_file","path":path,"content":content})
}
fn replace(path: &str, before: &str, after: &str) -> Value {
    json!({"type":"replace_file","path":path,"expected_sha256":hash(before),"content":after})
}
fn patch(text: String) -> Value {
    json!({"type":"patch","source":{"type":"inline","patch":text}})
}
fn change(path: &str, before: &str, after: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-{before}\n+{after}\n"
    )
}
fn add(path: &str, content: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1 @@\n+{content}\n"
    )
}
fn delete(path: &str, content: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\ndeleted file mode 100644\n--- a/{path}\n+++ /dev/null\n@@ -1 +0,0 @@\n-{content}\n"
    )
}
fn rename(from: &str, to: &str) -> String {
    format!(
        "diff --git a/{from} b/{to}\nsimilarity index 100%\nrename from {from}\nrename to {to}\n"
    )
}

#[test]
fn typed_api_reports_identical_replacements() {
    let dir = fixture();
    fs::write(dir.path().join("a.rs"), "same").unwrap();
    let input = Input {
        operations: vec![Operation::ReplaceFile {
            path: "a.rs".into(),
            expected_sha256: hash("same"),
            content: "same".into(),
        }],
    };
    let outcome = apply(dir.path(), &input);
    assert!(outcome.success, "{:?}", outcome.error);
    assert_eq!(outcome.affected_files, vec!["a.rs"]);
}

#[test]
fn sorting_normalization_deduplication_and_content_reversion() {
    let dir = fixture();
    fs::create_dir(dir.path().join("src")).unwrap();
    succeeds(
        dir.path(),
        vec![
            create("src/z.rs", "original\n"),
            create("src/é.rs", "unicode"),
            create("a.rs", "a"),
            patch(change("src/z.rs", "original", "temporary")),
            patch(change("src/z.rs", "temporary", "original")),
            replace("src//./z.rs", "original\n", "original\n"),
        ],
        &["a.rs", "src/z.rs", "src/é.rs"],
    );
}

#[test]
fn ordered_file_moves_deletions_and_recreations() {
    let cases = [
        (
            vec![
                replace("a.rs", "old", "new"),
                json!({"type":"move_file","from":"a.rs","to":"b.rs"}),
            ],
            vec!["b.rs"],
        ),
        (
            vec![
                replace("a.rs", "old", "new"),
                json!({"type":"delete_file","path":"a.rs"}),
            ],
            vec![],
        ),
        (
            vec![
                create("new.rs", "one"),
                json!({"type":"delete_file","path":"new.rs"}),
                create("new.rs", "two"),
            ],
            vec!["new.rs"],
        ),
        (
            vec![
                json!({"type":"move_file","from":"a.rs","to":"b.rs"}),
                create("a.rs", "reborn"),
            ],
            vec!["a.rs", "b.rs"],
        ),
    ];
    for (operations, expected) in cases {
        let dir = fixture();
        fs::write(dir.path().join("a.rs"), "old").unwrap();
        succeeds(dir.path(), operations, &expected);
    }
}

#[test]
fn directory_moves_include_untouched_files_at_final_locations() {
    let dir = fixture();
    fs::create_dir_all(dir.path().join("draft/nested")).unwrap();
    for path in [
        "draft/nested/untouched.rs",
        "draft/edit.rs",
        "draft/deleted.rs",
    ] {
        fs::write(dir.path().join(path), "old\n").unwrap();
    }
    fs::create_dir(dir.path().join("draft-old")).unwrap();
    fs::write(dir.path().join("draft-old/unrelated.rs"), "unrelated").unwrap();
    succeeds(
        dir.path(),
        vec![
            patch(change("draft/edit.rs", "old", "new")),
            json!({"type":"move_directory","from":"draft","to":"stage"}),
            json!({"type":"move_directory","from":"stage","to":"final"}),
            json!({"type":"delete_file","path":"final/deleted.rs"}),
            json!({"type":"move_file","from":"final/nested/untouched.rs","to":"final/last.rs"}),
            json!({"type":"create_directory","path":"draft"}),
            create("draft/reborn.rs", "reborn"),
        ],
        &["draft/reborn.rs", "final/edit.rs", "final/last.rs"],
    );
}

#[test]
fn multi_file_patches_delete_earlier_targets_and_report_rename_destinations() {
    let dir = fixture();
    for path in ["edit.rs", "gone.rs", "old.rs"] {
        fs::write(dir.path().join(path), "before\n").unwrap();
    }
    let multi = change("edit.rs", "before", "after")
        + &add("new.rs", "new")
        + &delete("gone.rs", "changed")
        + &rename("old.rs", "renamed.rs");
    succeeds(
        dir.path(),
        vec![
            replace("old.rs", "before\n", "before\n"),
            patch(change("gone.rs", "before", "changed")),
            patch(multi),
        ],
        &["edit.rs", "new.rs", "renamed.rs"],
    );
}

#[test]
fn patch_renames_and_deletions_compose_with_recreation_and_moves() {
    let dir = fixture();
    fs::write(dir.path().join("a.rs"), "before\n").unwrap();
    succeeds(
        dir.path(),
        vec![
            patch(rename("a.rs", "b.rs")),
            create("a.rs", "reborn"),
            patch(rename("b.rs", "c.rs")),
            json!({"type":"move_file","from":"c.rs","to":"d.rs"}),
        ],
        &["a.rs", "d.rs"],
    );
    succeeds(
        dir.path(),
        vec![
            patch(delete("d.rs", "before")),
            patch(add("d.rs", "after")),
            patch(delete("d.rs", "after")),
        ],
        &[],
    );
}

#[test]
fn patch_sources_are_read_after_creation_and_replacement() {
    let dir = fixture();
    let first = add("first.rs", "first");
    let second = add("second.rs", "second");
    let source = json!({"type":"patch","source":{"type":"file","path":"generated.patch"}});
    succeeds(
        dir.path(),
        vec![
            create("generated.patch", &first),
            source.clone(),
            replace("generated.patch", &first, &second),
            source,
            json!({"type":"delete_file","path":"generated.patch"}),
        ],
        &["first.rs", "second.rs"],
    );
    fs::write(dir.path().join("read-only.patch"), add("third.rs", "third")).unwrap();
    succeeds(
        dir.path(),
        vec![json!({"type":"patch","source":{"type":"file","path":"read-only.patch"}})],
        &["third.rs"],
    );
}
