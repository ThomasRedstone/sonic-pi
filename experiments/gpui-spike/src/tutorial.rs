//! Tutorial + examples browser data (Tier 2.1): chapters from
//! `etc/doc/tutorial/*.md` (filename-ordered, titled by their first line) and
//! runnable examples from `etc/examples/<category>/*.rb`. Pure loaders —
//! the pane in `main.rs` renders them.

use std::path::{Path, PathBuf};

pub struct Chapter {
    pub path: PathBuf,
    /// First non-empty line of the file (e.g. "1.1 Live Coding").
    pub title: String,
}

/// All tutorial chapters, in filename (= curriculum) order.
pub fn load_chapters(tutorial_dir: &Path) -> Vec<Chapter> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(tutorial_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "md"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let title = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| {
                    t.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())
                })
                .unwrap_or_else(|| {
                    path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
                });
            Chapter { path, title }
        })
        .collect()
}

pub struct Example {
    pub category: String,
    pub name: String,
    pub path: PathBuf,
}

/// All examples, grouped by their category directory (alphabetical), files
/// alphabetical within each.
pub fn load_examples(examples_dir: &Path) -> Vec<Example> {
    let mut out = Vec::new();
    let Ok(cats) = std::fs::read_dir(examples_dir) else { return out };
    let mut cat_dirs: Vec<PathBuf> =
        cats.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
    cat_dirs.sort();
    for cat in cat_dirs {
        let category = cat.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&cat)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|e| e == "rb"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for path in files {
            let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            out.push(Example { category: category.clone(), name, path });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn etc() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../etc")
    }

    #[test]
    fn loads_the_real_tutorial_in_order() {
        let chapters = load_chapters(&etc().join("doc/tutorial"));
        assert!(chapters.len() > 60, "expected the full curriculum, got {}", chapters.len());
        // Filename order == curriculum order; welcome chapter first.
        assert!(
            chapters[0].path.file_name().unwrap().to_string_lossy().starts_with("01"),
            "first chapter: {:?}",
            chapters[0].path
        );
        assert!(chapters.iter().all(|c| !c.title.is_empty()));
        // Titles come from content, not filenames.
        assert!(
            chapters.iter().any(|c| c.title.contains("Live Coding")),
            "expected a Live Coding chapter"
        );
    }

    #[test]
    fn loads_the_real_examples_grouped() {
        let examples = load_examples(&etc().join("examples"));
        assert!(examples.len() >= 30, "got {}", examples.len());
        assert!(examples.iter().any(|e| e.category == "apprentice"));
        assert!(examples.iter().all(|e| e.path.extension().unwrap() == "rb"));
        // Grouped: categories arrive contiguously (alphabetical).
        let cats: Vec<&str> = examples.iter().map(|e| e.category.as_str()).collect();
        let mut deduped = cats.clone();
        deduped.dedup();
        let mut sorted = deduped.clone();
        sorted.sort();
        assert_eq!(deduped, sorted, "categories should be contiguous + sorted");
    }

    #[test]
    fn missing_dirs_yield_empty_lists() {
        assert!(load_chapters(Path::new("/nonexistent")).is_empty());
        assert!(load_examples(Path::new("/nonexistent")).is_empty());
    }
}
