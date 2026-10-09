# executor

A small Rust CLI that applies an ordered list of Git patches and file operations
to the current working directory. Patch application uses `git2` (libgit2);
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
