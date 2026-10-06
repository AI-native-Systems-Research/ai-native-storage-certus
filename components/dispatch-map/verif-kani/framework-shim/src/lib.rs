//! See Cargo.toml. Real `component-core` receptacle + error, plus a structural
//! `define_component!`.

#[path = "../../../../../lib/component-framework/crates/component-core/src/error.rs"]
pub mod error;
#[path = "../../../../../lib/component-framework/crates/component-core/src/receptacle.rs"]
pub mod receptacle;

pub use receptacle::Receptacle;

/// Structural stand-in for `component_macros::define_component!`.
///
/// Emits `struct Name { pub <field>: <type>, ..., pub <recep>: Receptacle<dyn I + Send + Sync>, ... }`
/// (the real macro's user-field and receptacle field definitions, same names, same types, same
/// order) and `Name::new(fields...) -> Arc<Self>` with every receptacle disconnected, exactly as
/// the real constructor leaves them.
#[macro_export]
macro_rules! define_component {
    (
        $vis:vis $name:ident {
            version: $version:literal,
            provides: [ $($iface:ident),* $(,)? ],
            receptacles: { $($rname:ident : $riface:ident),* $(,)? },
            fields: { $($fname:ident : $fty:ty),* $(,)? },
        }
    ) => {
        $vis struct $name {
            __version: &'static str,
            $(pub $fname: $fty,)*
            $(pub $rname: $crate::receptacle::Receptacle<dyn $riface + Send + Sync>,)*
        }

        impl $name {
            $vis fn new($($fname: $fty),*) -> ::std::sync::Arc<Self> {
                ::std::sync::Arc::new(Self {
                    __version: $version,
                    $($fname,)*
                    $($rname: $crate::receptacle::Receptacle::new(),)*
                })
            }
        }
    };
}
