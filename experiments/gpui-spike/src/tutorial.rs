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

/// Rewrite a chapter's relative image URLs to absolute paths (resolved
/// against the chapter file's directory) so the markdown view's disk loader
/// finds them regardless of the process CWD. http(s) URLs pass through.
/// Pure → unit-tested.
pub fn absolutize_image_paths(md: &str, md_dir: &Path) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(start) = rest.find("![") {
        // Copy up to the image, then try to parse `![alt](url)`.
        let (before, at_image) = rest.split_at(start);
        out.push_str(before);
        let Some(close_alt) = at_image.find("](") else {
            out.push_str(at_image);
            return out;
        };
        let Some(end) = at_image[close_alt..].find(')') else {
            out.push_str(at_image);
            return out;
        };
        let url_start = close_alt + 2;
        let url_end = close_alt + end;
        let url = &at_image[url_start..url_end];
        out.push_str(&at_image[..url_start]);
        if url.contains("://") || Path::new(url).is_absolute() {
            out.push_str(url);
        } else {
            // Normalise the ../-heavy relative references without requiring
            // the file to exist (canonicalize would).
            let mut abs = md_dir.to_path_buf();
            for part in Path::new(url).components() {
                match part {
                    std::path::Component::ParentDir => {
                        abs.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => abs.push(other),
                }
            }
            out.push_str(&abs.to_string_lossy());
        }
        rest = &at_image[url_end..];
    }
    out.push_str(rest);
    out
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
    fn absolutizes_relative_image_urls() {
        let dir = Path::new("/repo/etc/doc/tutorial");
        // The real chapters use ../-heavy relative references.
        let md = "intro\n![sample graph](../../../etc/doc/images/tutorial/sample.png)\nafter";
        let out = absolutize_image_paths(md, dir);
        assert!(
            out.contains("![sample graph](/repo/etc/doc/images/tutorial/sample.png)"),
            "got: {out}"
        );
        assert!(out.starts_with("intro\n") && out.ends_with("\nafter"));

        // http URLs and absolute paths pass through; broken syntax is left alone.
        let md = "![a](https://x/y.png) ![b](/abs/p.png) ![c](broken";
        let out = absolutize_image_paths(md, dir);
        assert!(out.contains("(https://x/y.png)"));
        assert!(out.contains("(/abs/p.png)"));
        assert!(out.ends_with("![c](broken"));

        // Every real chapter's images resolve to files that exist.
        let tutorial_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../etc/doc/tutorial");
        let mut checked = 0;
        for ch in load_chapters(&tutorial_dir) {
            let raw = std::fs::read_to_string(&ch.path).unwrap();
            let abs = absolutize_image_paths(&raw, ch.path.parent().unwrap());
            for line in abs.lines().filter(|l| l.trim_start().starts_with("![")) {
                if let Some(url) = line.split("](").nth(1).and_then(|s| s.split(')').next()) {
                    if !url.contains("://") {
                        assert!(Path::new(url).exists(), "missing image: {url}");
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked >= 20, "expected the tutorial's images, checked {checked}");
    }

    #[test]
    fn missing_dirs_yield_empty_lists() {
        assert!(load_chapters(Path::new("/nonexistent")).is_empty());
        assert!(load_examples(Path::new("/nonexistent")).is_empty());
    }
}
