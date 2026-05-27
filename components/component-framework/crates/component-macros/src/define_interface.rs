use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{braced, Ident, Result, Token, TraitItemFn, Visibility};

/// Parsed input for `define_interface!`.
///
/// Expected syntax:
/// ```text
/// define_interface! {
///     [pub] IFoo {
///         fn method(&self, arg: Type) -> ReturnType;
///         ...
///     }
/// }
/// ```
pub(crate) struct InterfaceInput {
    vis: Visibility,
    name: Ident,
    methods: Vec<TraitItemFn>,
}

impl Parse for InterfaceInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let vis: Visibility = input.parse()?;
        let name: Ident = input.parse()?;

        let content;
        braced!(content in input);

        let mut methods = Vec::new();
        while !content.is_empty() {
            let method: TraitItemFn = content.parse()?;

            // Validate: must take &self
            let has_self_receiver = method
                .sig
                .inputs
                .iter()
                .any(|arg| matches!(arg, syn::FnArg::Receiver(_)));
            if !has_self_receiver {
                return Err(syn::Error::new_spanned(
                    &method.sig,
                    "interface methods must take `&self` as the first parameter",
                ));
            }

            // Validate: no &mut self
            for arg in &method.sig.inputs {
                if let syn::FnArg::Receiver(receiver) = arg {
                    if receiver.mutability.is_some() {
                        return Err(syn::Error::new_spanned(
                            receiver,
                            "interface methods must take `&self`, not `&mut self`; \
                             use interior mutability in implementations",
                        ));
                    }
                }
            }

            methods.push(method);
        }

        if methods.is_empty() {
            return Err(syn::Error::new(
                name.span(),
                format!("interface `{name}` must declare at least one method"),
            ));
        }

        // Consume optional trailing comma after the braced block
        let _ = input.parse::<Token![,]>();

        Ok(InterfaceInput { vis, name, methods })
    }
}

pub(crate) fn expand(input: InterfaceInput) -> TokenStream {
    let InterfaceInput { vis, name, methods } = input;

    let method_items: Vec<_> = methods
        .iter()
        .map(|m| {
            let attrs = &m.attrs;
            let sig = &m.sig;
            quote! { #(#attrs)* #sig; }
        })
        .collect();

    quote! {
        #vis trait #name: ::std::marker::Send + ::std::marker::Sync + 'static {
            #(#method_items)*
        }

        impl ::component_core::interface::Interface for dyn #name + Send + Sync {}
    }
}
