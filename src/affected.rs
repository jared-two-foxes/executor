use crate::{Operation, is_symlink_or_reparse_point};
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(crate) struct AffectedFiles {
    paths: BTreeSet<PathBuf>,
}

impl AffectedFiles {
    pub(crate) fn insert(&mut self, path: &Path) {
        self.paths.insert(path.components().collect());
    }

    pub(crate) fn remove(&mut self, path: &Path) {
        self.paths.remove(&path.components().collect::<PathBuf>());
    }

    pub(crate) fn record(&mut self, operation: &Operation) {
        match operation {
            Operation::CreateFile { path, .. }
            | Operation::ReplaceFile { path, .. }
            | Operation::EditFile { path, .. } => {
                self.insert(path);
            }
            Operation::DeleteFile { path } => self.remove(path),
            Operation::MoveFile { from, to } => {
                self.remove(from);
                self.insert(to);
            }
            _ => {}
        }
    }

    pub(crate) fn relocate(&mut self, from: &Path, to: &Path, files: Vec<PathBuf>) {
        let from: PathBuf = from.components().collect();
        self.paths.retain(|path| !path.starts_with(&from));
        for file in files {
            self.insert(&to.join(file));
        }
    }

    pub(crate) fn finish(self, root: &Path) -> Result<Vec<String>, String> {
        let mut files = Vec::new();
        for path in self.paths {
            if !surviving_regular_file(root, &path)? {
                continue;
            }
            // Resolve on-disk spelling so Windows case aliases deduplicate too.
            // Every component was inspected above; links are never candidates.
            #[cfg(windows)]
            let path = fs::canonicalize(root.join(&path))
                .map_err(|error| {
                    format!("Cannot resolve affected path {}: {error}", path.display())
                })?
                .strip_prefix(root)
                .map_err(|_| format!("Affected path escapes repository: {}", path.display()))?
                .to_path_buf();
            let components = path
                .components()
                .map(|component| {
                    component.as_os_str().to_str().ok_or_else(|| {
                        format!("Cannot report non-UTF-8 affected path: {}", path.display())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            files.push(components.join("/"));
        }
        files.sort_unstable();
        files.dedup();
        Ok(files)
    }
}

fn surviving_regular_file(root: &Path, relative: &Path) -> Result<bool, String> {
    let mut path = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        path.push(component);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "Cannot inspect affected path {}: {error}",
                    path.display()
                ));
            }
        };
        if is_symlink_or_reparse_point(&metadata) {
            return Ok(false);
        }
        if components.peek().is_none() {
            return Ok(metadata.is_file());
        }
        if !metadata.is_dir() {
            return Ok(false);
        }
    }
    Ok(false)
}

pub(crate) fn regular_descendants(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let path = directory.join(&relative);
        let entries = fs::read_dir(&path).map_err(|error| {
            format!(
                "Cannot enumerate affected directory {}: {error}",
                path.display()
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "Cannot enumerate affected directory {}: {error}",
                    path.display()
                )
            })?;
            let suffix = relative.join(entry.file_name());
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                format!(
                    "Cannot inspect affected path {}: {error}",
                    entry.path().display()
                )
            })?;
            if is_symlink_or_reparse_point(&metadata) {
                continue;
            }
            if metadata.is_file() {
                files.push(suffix);
            } else if metadata.is_dir() {
                pending.push(suffix);
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalization_excludes_missing_paths_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("directory")).unwrap();
        let mut affected = AffectedFiles::default();
        affected.insert(Path::new("missing"));
        affected.insert(Path::new("directory"));
        assert!(affected.finish(dir.path()).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn finalization_does_not_follow_symlink_ancestors_or_final_links() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("file"), "outside").unwrap();
        symlink(outside.path(), dir.path().join("link")).unwrap();
        symlink(outside.path().join("file"), dir.path().join("file")).unwrap();
        let mut affected = AffectedFiles::default();
        affected.insert(Path::new("link/file"));
        affected.insert(Path::new("file"));
        assert!(affected.finish(dir.path()).unwrap().is_empty());
    }
}
