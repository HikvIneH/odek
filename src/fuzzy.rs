//! ⌘P quick open: list project files (respecting .gitignore) and rank them
//! with nucleo, the matcher Helix uses.

use std::path::Path;

use ignore::WalkBuilder;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Stop listing past this many files so a stray giant folder can't eat memory.
pub const MAX_FILES: usize = 50_000;

/// Relative paths of every non-ignored file under `root`.
pub fn list_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | ".DS_Store")))
        .build();
    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            out.push(rel.to_string_lossy().into_owned());
            if out.len() >= MAX_FILES {
                break;
            }
        }
    }
    out
}

/// Indices into `files`, best match first, at most `limit`.
pub fn rank(files: &[String], query: &str, limit: usize) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..files.len().min(limit)).collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = files
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let mut score = pattern.score(Utf32Str::new(f, &mut buf), &mut matcher)?;
            // Prefer hits in the file name over hits spread across folders.
            let name = f.rsplit('/').next().unwrap_or(f);
            if let Some(s) = pattern.score(Utf32Str::new(name, &mut buf), &mut matcher) {
                score += s / 2;
            }
            Some((score, i))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| files[a.1].len().cmp(&files[b.1].len()))
    });
    scored.into_iter().take(limit).map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_beats_scattered_path_match() {
        let files: Vec<String> = [
            "internal/handlers/menu/admin.go",
            "web/src/pages/Reports.tsx",
            "web/src/pages/admin/Menu.tsx",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let hits = rank(&files, "menu", 10);
        assert_eq!(files[hits[0]], "web/src/pages/admin/Menu.tsx");
        assert_eq!(rank(&files, "reprt", 10)[0], 1);
        assert!(rank(&files, "zzz", 10).is_empty());
    }
}
