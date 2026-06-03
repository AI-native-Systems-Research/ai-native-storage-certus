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

/// Derive the factory symbol name from a dylib filename.
///
/// Convention: `lib<crate_name>.so` → `create_component_<crate_name>`
/// Example: `liblogger.so` → `create_component_logger`
///          `libblock_device_spdk_nvme.so` → `create_component_block_device_spdk_nvme`
fn derive_symbol_name(dylib_filename: &str) -> String {
    let stem = dylib_filename
        .strip_prefix("lib")
        .unwrap_or(dylib_filename)
        .strip_suffix(".so")
        .unwrap_or(dylib_filename);
    format!("create_component_{stem}")
}

/// Call the component factory on a loaded library to instantiate a component.
///
/// The factory symbol name is derived from `dylib_filename`:
/// `lib<name>.so` → `create_component_<name>`.
///
/// Catches panics from the factory function and converts them to errors.
///
/// # Errors
///
/// Returns an error if the symbol is not found or if the factory panics.
pub fn create_component(library: &Library, dylib_filename: &str) -> Result<ComponentRef, String> {
    let symbol_name = derive_symbol_name(dylib_filename);

    // SAFETY: The symbol has the Rust-ABI signature `fn() -> ComponentRef`.
    // We trust ABI compatibility (same toolchain) per project assumptions.
    let create: Symbol<fn() -> ComponentRef> =
        unsafe { library.get(symbol_name.as_bytes()) }
            .map_err(|e| format!("symbol '{}' not found: {e}", symbol_name))?;

    std::panic::catch_unwind(std::panic::AssertUnwindSafe(&*create))
        .map_err(|_| format!("{}() panicked", symbol_name))
}
