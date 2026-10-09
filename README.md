# executor

A small Rust CLI that applies an ordered sequence of Git patches to the current
working directory using `git2` (libgit2). It does not invoke Git or execute code,
build commands or scripts from the target repository.

Install from this checkout:

```sh
cargo install --path . --locked
```

Run from the **root of the target Git repository**:

```sh
executor apply /path/to/patches.json
cat /path/to/patches.json | executor apply -
```

The argument `-` reads JSON from stdin. The input supports inline diffs and
file references, in any order:

```json
{
  "patches": [
    {"type": "inline", "patch": "diff --git a/example.txt b/example.txt\n--- a/example.txt\n+++ b/example.txt\n@@ -1 +1 @@\n-original\n+updated\n"},
    {"type": "file", "path": "next.patch"}
  ]
}
```

File paths are resolved against the current working directory, even when the
JSON comes from elsewhere or stdin. File references are read when their turn
arrives. Plain strings in `patches` still work as shorthand for inline diffs.
The input has either `patches` or `operations`; there is no target repository
parameter. An empty list succeeds with zero applied items. An empty or malformed
patch fails.

For explicit filesystem changes, use an ordered `operations` list instead of
`patches`:

```json
{
  "operations": [
    {"type": "create_directory", "path": "notes"},
    {"type": "create_file", "path": "notes/draft.txt", "content": "Hello\n"},
    {"type": "move_file", "from": "notes/draft.txt", "to": "notes/final.txt"},
    {"type": "patch", "source": {"type": "inline", "patch": "diff --git a/example.txt b/example.txt\n--- a/example.txt\n+++ b/example.txt\n@@ -1 +1 @@\n-original\n+updated\n"}},
    {"type": "delete_file", "path": "notes/final.txt"},
    {"type": "move_directory", "from": "notes", "to": "archive"},
    {"type": "delete_directory", "path": "archive"}
  ]
}
```

The operation types are `patch`, `create_file`, `delete_file`, `move_file`,
`create_directory`, `delete_directory`, and `move_directory`. A `patch` source
may be inline, a file reference, or a plain diff string. File content is UTF-8
text. Creation requires the parent directory to exist and fails if the target
already exists. Moves fail if the destination exists. Directory deletion
requires an empty directory; it never removes contents recursively. Paths are
relative to the current repository root and receive the same traversal and
symlink checks as patch paths. Operations are not staged or committed.

Changes are applied sequentially to the working directory only. Existing staged,
uncommitted and untracked files are allowed; the index and commits are untouched.
A patch may depend on an earlier patch. On the first failure, executor stops and
leaves the working directory as it stands: no rollback, merge or conflict
resolution. `patches_applied` counts **fully completed patches**, not files or
hunks. Any changes left by a failed libgit2 application are also retained.

Stdout contains a JSON result, with exit code 0 on success and 1 on failure:

```json
{"success":true,"patches_applied":2}
```

```json
{"success":false,"patches_applied":1,"failed_patch":1,"error":"Patch could not be applied: ..."}
```

The `operations` input reports `operations_applied` and, on failure,
`failed_operation` instead. The count includes completed patch operations.
Like patches, operations stop at the first failure and retain earlier changes.

`failed_patch` is zero-based, including when a referenced patch file cannot be
read. Input, CLI and repository errors have
`patches_applied: 0` and omit `failed_patch` because no patch was attempted.
`--help` and `--version` display standard CLI text.

All source and destination paths are checked before each patch is applied.
Absolute paths, traversal, Git metadata paths and paths through existing
symlinks (including dangling links) are rejected. Symlink and submodule patches
are unsupported. Validation is repeated for each patch against the current
filesystem. Do not concurrently replace target paths while executor is running.

Run the integration tests with `cargo test --locked`.
