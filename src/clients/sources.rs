//! Resolve bounded configuration locations and physical identity. Home is never a project.
use super::descriptors::{Descriptor, RelativePath};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

pub(super) struct Source {
    pub path: PathBuf,
    pub project: bool,
}

pub fn identity(path: &Path) -> PathBuf {
    crate::platform::project_path(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn selected_project<'a>(
    home: &Path,
    project: &'a Path,
    selected: bool,
) -> Option<&'a Path> {
    (selected && identity(home) != identity(project)).then_some(project)
}

pub(crate) fn personal_paths(
    home: Option<&Path>,
    descriptor: &Descriptor,
    env: impl Fn(&str) -> Option<OsString>,
) -> Vec<PathBuf> {
    personal_paths_for_os(home, descriptor, env, std::env::consts::OS)
}

pub(super) fn personal_paths_for_os(
    home: Option<&Path>,
    descriptor: &Descriptor,
    env: impl Fn(&str) -> Option<OsString>,
    os: &str,
) -> Vec<PathBuf> {
    let mut defaults = Vec::new();
    let mut overrides = Vec::new();
    for location in &descriptor.personal {
        if location
            .os
            .as_deref()
            .is_some_and(|required| required != os)
        {
            continue;
        }
        if let Some(key) = &location.env {
            if let Some(base) = env(key).map(PathBuf::from) {
                overrides.push(match &location.path {
                    Some(path) => resolve(base, path),
                    None => base,
                });
            }
        } else if let (Some(home), Some(path)) = (home, &location.path) {
            defaults.push(resolve(home.to_path_buf(), path));
        }
    }
    // An override suppresses defaults even if the selected files do not exist.
    if overrides.is_empty() {
        defaults
    } else {
        overrides
    }
}

fn resolve(mut base: PathBuf, path: &RelativePath) -> PathBuf {
    match path {
        RelativePath::Text(path) => base.push(path),
        // Join each component so existing environment-based locations retain
        // their native separators in serialized paths on Windows.
        RelativePath::Components(parts) => base.extend(parts),
    }
    base
}

pub(super) fn paths(
    descriptor: &Descriptor,
    personal: Vec<PathBuf>,
    project: Option<&Path>,
) -> Vec<Source> {
    let personal = personal.into_iter().map(|path| Source {
        path,
        project: false,
    });
    let project = project.into_iter().flat_map(|root| {
        descriptor.project.iter().map(move |path| Source {
            path: root.join(path),
            project: true,
        })
    });
    // Prefer the personal scope when a project path aliases that same file.
    let mut seen = BTreeSet::new();
    personal
        .chain(project)
        .filter(|source| seen.insert(identity(&source.path)))
        .collect()
}

#[cfg(test)]
mod tests;
