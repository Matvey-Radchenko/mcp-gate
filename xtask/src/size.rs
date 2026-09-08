//! Mechanical source-size budgets, separate from semantic architecture review.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Quality {
    limits: Vec<Limit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Limit {
    path: PathBuf,
    max_lines: usize,
}

pub fn check(root: &Path) -> Result<()> {
    let text = fs::read_to_string(root.join("quality.toml")).context("Cannot read quality.toml")?;
    let quality: Quality = toml::from_str(&text).context("Invalid quality.toml")?;
    ensure!(
        !quality.limits.is_empty(),
        "No source-size budgets configured"
    );
    let mut violations = Vec::new();
    for limit in quality.limits {
        ensure!(limit.max_lines > 0, "Source-size budget must be positive");
        ensure!(
            !limit.path.as_os_str().is_empty()
                && limit
                    .path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "Budget path must be relative without traversal: {}",
            limit.path.display()
        );
        let mut files = Vec::new();
        collect(&root.join(&limit.path), &mut files)?;
        files.sort();
        ensure!(
            !files.is_empty(),
            "No Rust sources under {}",
            limit.path.display()
        );
        for path in files {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("Cannot read {}", path.display()))?;
            let count = source.lines().count();
            let relative = path.strip_prefix(root)?;
            if count > limit.max_lines {
                violations.push(format!(
                    "{}: {count} lines, limit {} — split by responsibility",
                    relative.display(),
                    limit.max_lines
                ));
            }
        }
    }
    ensure!(
        violations.is_empty(),
        "Source-size budget exceeded:\n{}",
        violations.join("\n")
    );
    println!("Rust source-size budgets passed");
    Ok(())
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("Cannot inspect {}", path.display()))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "Symlink in source tree: {}",
        path.display()
    );
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            collect(&entry?.path(), files)?;
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        files.push(path.to_path_buf());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(config: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("quality.toml"), config).unwrap();
        dir
    }

    #[test]
    fn exact_limit_passes_and_nested_overflow_fails() {
        let dir = fixture("[[limits]]\npath = 'src'\nmax_lines = 2\n");
        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        let source = dir.path().join("src/nested/lib.rs");
        fs::write(&source, "// comment\n\n").unwrap();
        check(dir.path()).unwrap();
        fs::write(&source, "// comment\n\nfn extra() {}").unwrap();
        let error = check(dir.path()).unwrap_err().to_string();
        let relative = Path::new("src").join("nested").join("lib.rs");
        assert!(error.contains(&format!("{}: 3 lines, limit 2", relative.display())));
    }

    #[test]
    fn invalid_or_empty_budgets_fail_closed() {
        for config in [
            "limits = []",
            "[[limits]]\npath = 'src'\nmax_lines = 0",
            "[[limits]]\npath = '../src'\nmax_lines = 2",
            "[[limits]]\npath = '/src'\nmax_lines = 2",
            "[[limits]]\npath = 'src'\nmax_lines = 2\nunknown = true",
        ] {
            let dir = fixture(config);
            assert!(check(dir.path()).is_err(), "Accepted {config}");
        }
    }

    #[test]
    fn missing_or_empty_source_directory_fails() {
        let dir = fixture("[[limits]]\npath = 'src'\nmax_lines = 2");
        assert!(check(dir.path()).is_err());
        fs::create_dir(dir.path().join("src")).unwrap();
        assert!(check(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_do_not_bypass_the_budget() {
        let dir = fixture("[[limits]]\npath = 'src'\nmax_lines = 2");
        fs::create_dir(dir.path().join("src")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("src"), dir.path().join("src/loop")).unwrap();
        assert!(
            check(dir.path())
                .unwrap_err()
                .to_string()
                .contains("Symlink")
        );
    }
}
