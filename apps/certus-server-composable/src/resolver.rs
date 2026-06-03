//! Dylib path resolution for component shared libraries.
//!
//! Resolves dylib filenames to absolute filesystem paths using a configurable
//! search path list. The `CERTUS_LIB_PATH` environment variable is prepended
//! to search paths defined in the JSON configuration.

use std::path::PathBuf;

/// Resolved dylib path for a single component.
#[derive(Debug, Clone)]
pub struct ResolvedDylib {
    pub component_name: String,
    pub path: PathBuf,
}

/// Build the effective search path list.
///
/// Order: CERTUS_LIB_PATH entries (colon-separated) first, then
/// paths from the JSON configuration.
pub fn build_search_paths(config_paths: &[String]) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(env_path) = std::env::var("CERTUS_LIB_PATH") {
        for entry in env_path.split(':') {
            let trimmed = entry.trim();
            if !trimmed.is_empty() {
                paths.push(PathBuf::from(trimmed));
            }
        }
    }

    for p in config_paths {
        paths.push(PathBuf::from(p));
    }

    paths
}

/// Resolve a single dylib filename to an absolute path.
///
/// If `absolute_path` is `Some`, uses it directly (verifying it exists).
/// Otherwise, searches the provided directories in order for `dylib_filename`.
///
/// # Errors
///
/// Returns an error string if the dylib cannot be found in any search path.
pub fn resolve_dylib(
    dylib_filename: &str,
    absolute_path: Option<&str>,
    search_paths: &[PathBuf],
) -> Result<PathBuf, String> {
    if let Some(abs) = absolute_path {
        let p = PathBuf::from(abs);
        if p.exists() && p.is_file() {
            return Ok(p);
        }
        return Err(format!(
            "absolute path '{}' does not exist or is not a file",
            abs
        ));
    }

    for dir in search_paths {
        let candidate = dir.join(dylib_filename);
        if candidate.exists() && candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "'{}' not found in search paths: {:?}",
        dylib_filename,
        search_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
    ))
}

/// Resolve all component dylib paths from a configuration.
///
/// Verifies that ALL dylibs are accessible BEFORE any component is loaded.
///
/// # Errors
///
/// Returns a list of all resolution failures (not just the first one).
pub fn resolve_all_dylibs(
    components: &[crate::config::ComponentSpec],
    search_paths: &[PathBuf],
) -> Result<Vec<ResolvedDylib>, Vec<String>> {
    let mut resolved = Vec::new();
    let mut errors = Vec::new();

    for comp in components {
        match resolve_dylib(&comp.dylib, comp.path.as_deref(), search_paths) {
            Ok(path) => resolved.push(ResolvedDylib {
                component_name: comp.name.clone(),
                path,
            }),
            Err(e) => errors.push(format!("component '{}': {e}", comp.name)),
        }
    }

    if errors.is_empty() {
        Ok(resolved)
    } else {
        Err(errors)
    }
}
