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
```

The input contains complete Git unified diffs as JSON strings:

```json
{
  "patches": [
    "diff --git a/example.txt b/example.txt\n--- a/example.txt\n+++ b/example.txt\n@@ -1 +1 @@\n-original\n+updated\n"
  ]
}
```

Only `patches` is accepted; there is no target repository parameter. An empty
list succeeds with zero applied patches. An empty or malformed patch fails.

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

`failed_patch` is zero-based. Input, CLI and repository errors have
`patches_applied: 0` and omit `failed_patch` because no patch was attempted.
`--help` and `--version` display standard CLI text.

All source and destination paths are checked before each patch is applied.
Absolute paths, traversal, Git metadata paths and paths through existing
symlinks (including dangling links) are rejected. Symlink and submodule patches
are unsupported. Validation is repeated for each patch against the current
filesystem. Do not concurrently replace target paths while executor is running.

Run the integration tests with `cargo test --locked`.
