# executor

A Rust CLI and reusable library that apply an ordered list of Git patches and
file operations to a Git working tree. Patch application uses `git2` (libgit2);
executor does not invoke Git or run code from the target repository.

```sh
cargo install --path . --locked
cd /path/to/git-repository
executor apply /path/to/input.json
cat /path/to/input.json | executor apply -
```

Run from the **root of the target Git repository**. The `-` argument reads JSON
from stdin. The sole input format is an `operations` array, executed in order:

```json
{
  "operations": [
    {"type": "create_file", "path": "notes.txt", "content": "Hello\n"},
    {"type": "patch", "source": {"type": "inline", "patch": "diff --git a/example.txt b/example.txt\n--- a/example.txt\n+++ b/example.txt\n@@ -1 +1 @@\n-original\n+updated\n"}},
    {"type": "replace_file", "path": "notes.txt", "expected_sha256": "66a045b452102c59d840ec097d59d9467e13a3f34f6494e539ffd32c1bb35f18", "content": "Updated\n"},
    {"type": "patch", "source": {"type": "file", "path": "next.patch"}},
    {"type": "move_file", "from": "notes.txt", "to": "finished.txt"}
  ]
}
```

Operations can interleave patches with `create_file`, `replace_file`,
`delete_file`, `move_file`, `create_directory`, `delete_directory`, and
`move_directory`. Patch sources must be `inline` or `file`. Referenced patch
files are read when their operation runs, with paths relative to the current
working directory. File content is UTF-8 text. `replace_file` requires the
SHA-256 of the file's current bytes as 64 lowercase hex characters, fails
without changing the file if that hash differs, and preserves permissions.

Creation requires the parent directory to exist and fails if the target exists.
Moves fail if the destination exists. Directory deletion requires an empty
directory. An empty operations list succeeds. A patch with no file changes fails.

Changes affect the working directory only. Existing staged, uncommitted and
untracked files are allowed; the index and commits are untouched. On failure,
executor stops and retains all changes already made, including any changes
left by a failed patch. It does not roll back, merge or resolve conflicts.

Stdout contains JSON; exit code 0 indicates success and nonzero indicates
failure:

```json
{"success":true,"operations_applied":2}
```

```json
{"success":false,"operations_applied":1,"failed_operation":1,"error":"Patch could not be applied: ..."}
```

`operations_applied` counts fully completed operations. `failed_operation` is
zero-based and is omitted for invalid input, CLI and repository errors, which
report `operations_applied: 0`. `--help` and `--version` print standard CLI
text.

All source and destination paths are checked before each patch is applied.
Absolute paths, traversal, Git metadata paths and paths through existing
symlinks (including dangling links) are rejected. Symlink and submodule patches
are unsupported. Validation is repeated for each operation against the current
filesystem. Do not concurrently replace target paths while executor is running.

Run the integration tests with `cargo test --locked`.

## Rust library integration

The `executor` package now exports a library alongside its existing CLI.
Other Rust projects can depend on the repository directly:

```toml
[dependencies]
executor = { git = "https://github.com/jared-two-foxes/executor" }
```

Call the library with an explicit Git repository root; it does not change the
process working directory or spawn a separate executor process:

```rust
use executor::{Input, Operation, apply, apply_json};
use std::path::Path;

let repo_root = Path::new("/path/to/git-repository");
let input = Input {
    operations: vec![Operation::CreateFile {
        path: "hello.txt".into(),
        content: "Hello from Rust\\n".into(),
    }],
};
let outcome = apply(repo_root, &input);
assert!(outcome.success, "{:?}", outcome.error);

// Or pass the existing JSON protocol directly:
let outcome = apply_json(repo_root, br#"{"operations":[]}"#);
assert!(outcome.success);
```

`Input`, `Operation`, `PatchSource`, and `Outcome` are public types.
`apply_json(root, bytes)` parses the same strict JSON as the CLI;
`apply(root, &input)` accepts typed operations. Both return `Outcome` with
`success`, `operations_applied`, `failed_operation`, and `error` fields.
Invalid JSON or repository paths return a failed outcome without applying
operations. If a later operation fails, earlier operations remain applied.
Relative patch-file sources are resolved against the supplied repository root.

The CLI still accepts `executor apply <file>` and `executor apply -` with
the same output JSON and exit-code contract. Existing CLI integration tests
continue to exercise that compatibility; `tests/library.rs` exercises the
in-process API.
