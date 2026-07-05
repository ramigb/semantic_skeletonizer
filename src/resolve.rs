//! Import-specifier resolution.
//!
//! `FsResolver` handles relative specifiers against the importing file and
//! classifies bare specifiers as external packages. tsconfig `paths` aliases
//! are out of scope for now — implement a new `Resolver` for them.

use std::path::Path;

use crate::graph::{canonical_key, is_skeleton_target};

#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Resolved to a canonical graph key inside the repo.
    Internal(String),
    /// Bare specifier: external package name (`react`, `@scope/pkg`).
    External(String),
    /// Relative specifier that doesn't point at a known source file.
    Unresolved,
}

pub trait Resolver: Send + Sync {
    fn resolve(&self, root: &Path, importer_key: &str, spec: &str) -> Resolution;
}

pub struct FsResolver;

fn package_name(spec: &str) -> String {
    let mut segments = spec.split('/');
    match segments.next() {
        Some(scope) if scope.starts_with('@') => match segments.next() {
            Some(name) => format!("{}/{}", scope, name),
            None => scope.to_string(),
        },
        Some(first) => first.to_string(),
        None => spec.to_string(),
    }
}

impl Resolver for FsResolver {
    fn resolve(&self, root: &Path, importer_key: &str, spec: &str) -> Resolution {
        if !spec.starts_with("./") && !spec.starts_with("../") && spec != "." && spec != ".." {
            return Resolution::External(package_name(spec));
        }

        let base = Path::new(importer_key)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let raw = base.join(spec);
        let raw_str = raw.to_string_lossy();

        let candidates = [
            format!("{}.ts", raw_str),
            format!("{}.tsx", raw_str),
            format!("{}/index.ts", raw_str),
            format!("{}/index.tsx", raw_str),
            raw_str.to_string(),
        ];

        for cand in &candidates {
            if let Some(key) = canonical_key(root, Path::new(cand)) {
                let abs = root.join(&key);
                if abs.is_file() && is_skeleton_target(&abs) {
                    return Resolution::Internal(key);
                }
            }
        }
        Resolution::Unresolved
    }
}
