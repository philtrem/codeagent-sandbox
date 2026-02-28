use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Wraps a compiled set of gitignore rules and exposes a simple
/// `is_ignored` predicate for relative paths.
pub struct GitignoreFilter {
    matcher: Gitignore,
}

impl GitignoreFilter {
    /// Build a filter by discovering `.gitignore` files under `working_root`
    /// and loading `.git/info/exclude` if present.
    ///
    /// Returns `None` when no gitignore sources are found (nothing to filter).
    pub fn build(working_root: &Path) -> Option<Self> {
        let mut builder = GitignoreBuilder::new(working_root);
        let mut found_sources = false;

        // .git/info/exclude
        let exclude_path = working_root.join(".git").join("info").join("exclude");
        if exclude_path.is_file() {
            builder.add(&exclude_path);
            found_sources = true;
        }

        // Walk the tree to discover all .gitignore files
        Self::discover_gitignore_files(working_root, &mut builder, &mut found_sources);

        if !found_sources {
            return None;
        }

        let matcher = builder.build().ok()?;
        if matcher.is_empty() {
            return None;
        }

        Some(Self { matcher })
    }

    /// Recursively discover `.gitignore` files and add them to the builder.
    fn discover_gitignore_files(
        dir: &Path,
        builder: &mut GitignoreBuilder,
        found_sources: &mut bool,
    ) {
        let gitignore_path = dir.join(".gitignore");
        if gitignore_path.is_file() {
            builder.add(&gitignore_path);
            *found_sources = true;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Skip the .git directory itself
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            Self::discover_gitignore_files(&path, builder, found_sources);
        }
    }

    /// Check whether a relative (forward-slash) path is ignored.
    /// Uses `matched_path_or_any_parents` so that files inside an ignored
    /// directory are also considered ignored (matching real Git semantics).
    pub fn is_ignored(&self, relative_path: &str, is_dir: bool) -> bool {
        self.matcher
            .matched_path_or_any_parents(relative_path, is_dir)
            .is_ignore()
    }
}
