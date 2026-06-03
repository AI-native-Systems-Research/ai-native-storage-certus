//! Dynamic library loading for component instantiation.
//!
//! Loads `.so` files at runtime using `libloading` and calls the
//! `create_component()` factory function to obtain a `ComponentRef`.
//! Panics from component factories are caught at the boundary.

use std::path::Path;
use std::sync::Arc;

use component_core::component_ref::ComponentRef;
use libloading::{Library, Symbol};

/// A loaded dynamic library paired with its filesystem path.
pub struct LoadedLibrary {
    pub library: Arc<Library>,
    #[allow(dead_code)]
    pub path: String,
}

/// Load a shared library from the given path.
///
/// # Safety
///
/// The caller must ensure the `.so` was built with the same Rust toolchain
/// and links against the same shared crates (component-core).
///
/// # Errors
///
/// Returns an error if the library cannot be opened.
pub fn load_library(path: &Path) -> Result<LoadedLibrary, String> {
    // SAFETY: We trust the .so was built with the same compiler and links
    // against the same shared crates (component-core). ABI compatibility
    // is the operator's responsibility per project assumptions.
    let library = unsafe { Library::new(path) }
        .map_err(|e| format!("failed to load '{}': {e}", path.display()))?;

    Ok(LoadedLibrary {
        library: Arc::new(library),
        path: path.display().to_string(),
    })
}

/// Call `create_component()` on a loaded library to instantiate a component.
///
/// Catches panics from the factory function and converts them to errors.
///
/// # Errors
///
/// Returns an error if the `create_component` symbol is not found or if
/// the factory function panics.
pub fn create_component(library: &Library) -> Result<ComponentRef, String> {
    // SAFETY: The symbol has the Rust-ABI signature `fn() -> ComponentRef`.
    // We trust ABI compatibility (same toolchain) per project assumptions.
    let create: Symbol<fn() -> ComponentRef> = unsafe { library.get(b"create_component") }
        .map_err(|e| format!("symbol 'create_component' not found: {e}"))?;

    std::panic::catch_unwind(std::panic::AssertUnwindSafe(&*create))
        .map_err(|_| "create_component() panicked".to_string())
}
