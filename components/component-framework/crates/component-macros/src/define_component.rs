use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{braced, Ident, LitStr, Result, Token, Type, Visibility};

/// Parsed input for `define_component!`.
///
/// Expected syntax:
/// ```text
/// define_component! {
///     [pub] MyComponent {
///         version: "1.0.0",
///         provides: [IStorage, ISerializable],
///         [receptacles: {
///             logger: ILogger,
///             storage: IStorage,
///         },]
///         [fields: {
///             data: HashMap<String, Vec<u8>>,
///         },]
///     }
/// }
/// ```
pub(crate) struct ComponentInput {
    vis: Visibility,
    name: Ident,
    version: LitStr,
    provides: Vec<Ident>,
    receptacles: Vec<(Ident, Ident)>,
    fields: Vec<(Ident, Type)>,
}

impl Parse for ComponentInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let vis: Visibility = input.parse()?;
        let name: Ident = input.parse()?;

        let content;
        braced!(content in input);

        // Parse version: "x.y.z",
        let version_key: Ident = content.parse()?;
        if version_key != "version" {
            return Err(syn::Error::new_spanned(
                &version_key,
                "expected `version` as the first field",
            ));
        }
        content.parse::<Token![:]>()?;
        let version: LitStr = content.parse()?;
        content.parse::<Token![,]>()?;

        // Parse provides: [Interface1, Interface2],
        let provides_key: Ident = content.parse()?;
        if provides_key != "provides" {
            return Err(syn::Error::new_spanned(
                &provides_key,
                "expected `provides` after `version`",
            ));
        }
        content.parse::<Token![:]>()?;

        let provides_content;
        syn::bracketed!(provides_content in content);
        let provides: Punctuated<Ident, Token![,]> =
            provides_content.parse_terminated(Ident::parse, Token![,])?;
        let provides: Vec<Ident> = provides.into_iter().collect();
        content.parse::<Token![,]>()?;

        // Parse optional receptacles: { name: Interface, ... },
        let mut receptacles = Vec::new();
        let mut fields = Vec::new();

        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![:]>()?;

            if key == "receptacles" {
                let recep_content;
                braced!(recep_content in content);
                while !recep_content.is_empty() {
                    let recep_name: Ident = recep_content.parse()?;
                    recep_content.parse::<Token![:]>()?;
                    let recep_iface: Ident = recep_content.parse()?;
                    receptacles.push((recep_name, recep_iface));
                    if recep_content.peek(Token![,]) {
                        recep_content.parse::<Token![,]>()?;
                    }
                }
            } else if key == "fields" {
                let fields_content;
                braced!(fields_content in content);
                while !fields_content.is_empty() {
                    let field_name: Ident = fields_content.parse()?;
                    fields_content.parse::<Token![:]>()?;
                    let field_type: Type = fields_content.parse()?;
                    fields.push((field_name, field_type));
                    if fields_content.peek(Token![,]) {
                        fields_content.parse::<Token![,]>()?;
                    }
                }
            } else {
                return Err(syn::Error::new_spanned(
                    &key,
                    format!("unexpected field `{key}`; expected `receptacles` or `fields`"),
                ));
            }

            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }

        Ok(ComponentInput {
            vis,
            name,
            version,
            provides,
            receptacles,
            fields,
        })
    }
}

pub(crate) fn expand(input: ComponentInput) -> TokenStream {
    let ComponentInput {
        vis,
        name,
        version,
        provides,
        receptacles,
        fields,
    } = input;

    // Generate struct fields for receptacles
    let recep_field_defs: Vec<_> = receptacles
        .iter()
        .map(|(rname, iface)| {
            quote! {
                pub #rname: ::component_core::receptacle::Receptacle<dyn #iface + Send + Sync>
            }
        })
        .collect();

    // Generate user field definitions
    let user_field_defs: Vec<_> = fields
        .iter()
        .map(|(fname, ftype)| {
            quote! { pub #fname: #ftype }
        })
        .collect();

    // Generate new() constructor parameter list (user fields only)
    let constructor_params: Vec<_> = fields
        .iter()
        .map(|(fname, ftype)| {
            quote! { #fname: #ftype }
        })
        .collect();

    // Generate field initialization in new()
    let user_field_inits: Vec<_> = fields
        .iter()
        .map(|(fname, _)| {
            quote! { #fname }
        })
        .collect();

    let recep_field_inits: Vec<_> = receptacles
        .iter()
        .map(|(rname, _)| {
            quote! {
                #rname: ::component_core::receptacle::Receptacle::new()
            }
        })
        .collect();

    // Generate InterfaceMap population in new()
    let interface_map_inserts: Vec<_> = provides
        .iter()
        .map(|iface| {
            let iface_name = iface.to_string();
            quote! {
                {
                    let arc: ::std::sync::Arc<dyn #iface + Send + Sync> =
                        ::std::sync::Arc::clone(&self_arc) as ::std::sync::Arc<dyn #iface + Send + Sync>;
                    __interface_map.insert(
                        ::std::any::TypeId::of::<
                            ::std::sync::Arc<dyn #iface + ::std::marker::Send + ::std::marker::Sync>
                        >(),
                        #iface_name,
                        Box::new(arc),
                    );
                }
            }
        })
        .collect();

    // Generate InterfaceInfo for IUnknown itself
    let iunknown_insert = quote! {
        {
            let arc: ::std::sync::Arc<dyn ::component_core::iunknown::IUnknown> =
                ::std::sync::Arc::clone(&self_arc) as ::std::sync::Arc<dyn ::component_core::iunknown::IUnknown>;
            __interface_map.insert(
                ::std::any::TypeId::of::<
                    ::std::sync::Arc<dyn ::component_core::iunknown::IUnknown>
                >(),
                "IUnknown",
                Box::new(arc),
            );
        }
    };

    // Generate provided_interfaces info (static)
    let interface_info_items: Vec<_> = provides
        .iter()
        .map(|iface| {
            let iface_name = iface.to_string();
            quote! {
                ::component_core::interface::InterfaceInfo {
                    type_id: ::std::any::TypeId::of::<
                        ::std::sync::Arc<dyn #iface + ::std::marker::Send + ::std::marker::Sync>
                    >(),
                    name: #iface_name,
                }
            }
        })
        .collect();

    // Generate ReceptacleInfo items
    let receptacle_info_items: Vec<_> = receptacles
        .iter()
        .map(|(rname, iface)| {
            let rname_str = rname.to_string();
            let iface_str = iface.to_string();
            quote! {
                ::component_core::interface::ReceptacleInfo {
                    type_id: ::std::any::TypeId::of::<
                        ::std::sync::Arc<dyn #iface + ::std::marker::Send + ::std::marker::Sync>
                    >(),
                    name: #rname_str,
                    interface_name: #iface_str,
                }
            }
        })
        .collect();

    // Generate connect_receptacle_raw match arms.
    // Each arm queries the provider (via IUnknown) for the expected interface
    // type, then connects the resulting Arc to the receptacle.
    let receptacle_connect_arms: Vec<_> = receptacles
        .iter()
        .map(|(rname, iface)| {
            let rname_str = rname.to_string();
            let iface_str = iface.to_string();
            quote! {
                #rname_str => {
                    let type_id = ::std::any::TypeId::of::<
                        ::std::sync::Arc<dyn #iface + ::std::marker::Send + ::std::marker::Sync>
                    >();
                    let any_ref = provider.query_interface_raw(type_id)
                        .ok_or_else(|| ::component_core::error::RegistryError::BindingFailed {
                            detail: format!(
                                "provider does not implement '{}' needed by receptacle '{}'",
                                #iface_str, #rname_str
                            ),
                        })?;
                    let arc = any_ref
                        .downcast_ref::<::std::sync::Arc<dyn #iface + ::std::marker::Send + ::std::marker::Sync>>()
                        .ok_or_else(|| ::component_core::error::RegistryError::BindingFailed {
                            detail: format!("type mismatch for receptacle '{}'", #rname_str),
                        })?;
                    self.#rname.connect(::std::sync::Arc::clone(arc)).map_err(|e| {
                        ::component_core::error::RegistryError::BindingFailed {
                            detail: format!("receptacle '{}': {}", #rname_str, e),
                        }
                    })
                }
            }
        })
        .collect();

    // Generate new_default() when there are user fields.
    // Calls new() with Default::default() for each field.
    let new_default_method = if !fields.is_empty() {
        let default_args: Vec<_> = fields
            .iter()
            .map(|(_, ftype)| {
                quote! { <#ftype as ::std::default::Default>::default() }
            })
            .collect();
        Some(quote! {
            /// Create a new instance with all fields set to their default values.
            ///
            /// Requires all user-defined fields to implement [`Default`].
            #vis fn new_default() -> ::std::sync::Arc<Self> {
                Self::new(#(#default_args),*)
            }
        })
    } else {
        None
    };

    // The init_interfaces method name (private, snake_case to avoid warning)
    let name_snake = name.to_string().to_lowercase();
    let init_method = format_ident!("__init_interfaces_{}", name_snake);

    quote! {
        #vis struct #name {
            __interface_map: ::component_core::component::InterfaceMap,
            __interface_info: ::std::vec::Vec<::component_core::interface::InterfaceInfo>,
            __receptacle_info: ::std::vec::Vec<::component_core::interface::ReceptacleInfo>,
            __version: &'static str,
            #(#recep_field_defs,)*
            #(#user_field_defs,)*
        }

        impl #name {
            /// Create a new instance of this component.
            ///
            /// The returned `Arc` is required because the component stores
            /// `Arc` references to itself for interface queries.
            #vis fn new(#(#constructor_params),*) -> ::std::sync::Arc<Self> {
                let component = Self {
                    __interface_map: ::component_core::component::InterfaceMap::new(),
                    __interface_info: Vec::new(),
                    __receptacle_info: Vec::new(),
                    __version: #version,
                    #(#recep_field_inits,)*
                    #(#user_field_inits,)*
                };
                let self_arc = ::std::sync::Arc::new(component);
                #init_method(&self_arc);
                self_arc
            }

            #new_default_method
        }

        /// Initialize the interface map after Arc construction.
        /// This is safe because we have exclusive access during construction.
        fn #init_method(self_arc: &::std::sync::Arc<#name>) {
            let ptr = ::std::sync::Arc::as_ptr(self_arc) as *mut #name;
            // SAFETY: We have the only Arc reference at this point during
            // construction, so no other thread can observe the mutation.
            // The InterfaceMap, interface_info, and receptacle_info fields
            // are being initialized before the Arc is shared.
            unsafe {
                let component = &mut *ptr;
                let mut __interface_map = ::component_core::component::InterfaceMap::new();

                #(#interface_map_inserts)*
                #iunknown_insert

                component.__interface_map = __interface_map;

                component.__interface_info = vec![
                    #(#interface_info_items,)*
                    ::component_core::interface::InterfaceInfo {
                        type_id: ::std::any::TypeId::of::<
                            ::std::sync::Arc<dyn ::component_core::iunknown::IUnknown>
                        >(),
                        name: "IUnknown",
                    },
                ];

                component.__receptacle_info = vec![
                    #(#receptacle_info_items,)*
                ];
            }
        }

        impl ::component_core::iunknown::IUnknown for #name {
            fn query_interface_raw(
                &self,
                id: ::std::any::TypeId,
            ) -> Option<&(dyn ::std::any::Any + Send + Sync)> {
                self.__interface_map.lookup(id)
            }

            fn version(&self) -> &str {
                self.__version
            }

            fn provided_interfaces(&self) -> &[::component_core::interface::InterfaceInfo] {
                &self.__interface_info
            }

            fn receptacles(&self) -> &[::component_core::interface::ReceptacleInfo] {
                &self.__receptacle_info
            }

            fn connect_receptacle_raw(
                &self,
                receptacle_name: &str,
                provider: &dyn ::component_core::iunknown::IUnknown,
            ) -> Result<(), ::component_core::error::RegistryError> {
                match receptacle_name {
                    #(#receptacle_connect_arms)*
                    _ => Err(::component_core::error::RegistryError::BindingFailed {
                        detail: format!("unknown receptacle: {}", receptacle_name),
                    }),
                }
            }
        }

        // SAFETY: All fields are Send + Sync (InterfaceMap uses HashMap + Box<dyn Any + Send + Sync>,
        // Receptacle uses RwLock, user fields must be Send + Sync by trait bounds on provided interfaces).
        unsafe impl Send for #name {}
        unsafe impl Sync for #name {}
    }
}
