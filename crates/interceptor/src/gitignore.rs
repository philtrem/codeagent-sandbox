use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Build a compiled gitignore matcher by discovering `.gitignore` files under
/// `working_root` and loading `.git/info/exclude` if present.
///
/// Returns `None` when no gitignore sources are found (nothing to filter).
pub fn build_gitignore(working_root: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(working_root);
    let mut found_sources = false;

    // .git/info/exclude
    let exclude_path = working_root.join(".git").join("info").join("exclude");
    if exclude_path.is_file() {
        builder.add(&exclude_path);
        found_sources = true;
    }

    // Walk the tree to discover all .gitignore files
    discover_gitignore_files(working_root, &mut builder, &mut found_sources);

    if !found_sources {
        return None;
    }

    let matcher = builder.build().ok()?;
    if matcher.is_empty() {
        return None;
    }

    Some(matcher)
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
        discover_gitignore_files(&path, builder, found_sources);
    }
}
