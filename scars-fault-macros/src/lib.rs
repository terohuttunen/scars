//! Procedural macros for the `scars-fault` crate.
//!
//! This crate provides procedural macros for the `scars-fault` crate:
//! - `#[derive(Fault)]`: Derives the `Fault` trait with custom formatting
//! - `#[fault_handler]`: Marks a function as the error handler
//! - `fault!`: Macro for raising faults
//!
//! Two formatting backends, selected by feature on this crate (forwarded
//! from `scars-fault`):
//!
//! - `defmt` (embedded targets): the derive emits `defmt::Format` plus the
//!   dyn-safe `Fault::defmt_format` shim. Format strings flow through
//!   `defmt::write!`.
//! - `display` (host/sim targets): the derive emits `core::fmt::Display`.
//!   Format strings flow through `core::write!`.
//!
//! The two features are mutually exclusive — picking both is a build error
//! in `scars-fault`.
//!
//! # Custom Error Formatting
//!
//! The derive macro supports custom formatting for both structs and enums:
//!
//! ```rust
//! use scars_fault::Fault;
//!
//! #[derive(Debug, Fault)]
//! #[fault("Invalid configuration: {field} = {value}")]
//! struct ConfigError<'a> {
//!     field: &'a str,
//!     value: &'a str,
//! }
//!
//! #[derive(Debug, Fault)]
//! enum MyError<'a> {
//!     #[fault("Invalid input: {value}")]
//!     InvalidInput { value: &'a str },
//!     #[fault("Timeout after {ms}ms")]
//!     Timeout { ms: u32 },
//!     #[fault("Connection failed: {reason}")]
//!     ConnectionFailed { reason: &'a str },
//! }
//! ```
//!
//! The format strings support both `{}` and `{:?}` for field values:
//!
//! ```rust
//! use scars_fault::Fault;
//!
//! #[derive(Debug, Fault)]
//! enum DebugError {
//!     #[fault("Debug value: {value:?}")]
//!     Debug { value: Vec<u8> },
//! }
//! ```

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

#[derive(Clone, Copy)]
enum Backend {
    Defmt,
    Display,
}

#[cfg(all(feature = "defmt", not(feature = "display")))]
const BACKEND: Backend = Backend::Defmt;
#[cfg(all(feature = "display", not(feature = "defmt")))]
const BACKEND: Backend = Backend::Display;
#[cfg(all(feature = "defmt", feature = "display"))]
compile_error!("scars-fault-macros: features `defmt` and `display` are mutually exclusive");
#[cfg(not(any(feature = "defmt", feature = "display")))]
compile_error!("scars-fault-macros: enable one of features `defmt` or `display`");

/// Derives the `Fault` trait and a matching formatting impl for a type.
///
/// Under the `defmt` backend the derive emits a `defmt::Format` impl plus
/// the dyn-safe `Fault::defmt_format` shim. Under the `display` backend it
/// emits a `core::fmt::Display` impl.
///
/// # Custom Formatting
///
/// You can customize the error message using the `#[fault]` attribute:
///
/// ```rust
/// use scars_fault::Fault;
///
/// #[derive(Debug, Fault)]
/// #[fault("Invalid configuration: {field} = {value}")]
/// struct ConfigError<'a> {
///     field: &'a str,
///     value: &'a str,
/// }
/// ```
///
/// For enums, you can customize each variant:
///
/// ```rust
/// use scars_fault::Fault;
///
/// #[derive(Debug, Fault)]
/// enum MyError<'a> {
///     #[fault("Invalid input: {value}")]
///     InvalidInput { value: &'a str },
///     #[fault("Timeout after {ms}ms")]
///     Timeout { ms: u32 },
/// }
/// ```
///
/// The format strings support both `{}` and `{:?}` for field values:
///
/// ```rust
/// use scars_fault::Fault;
///
/// #[derive(Debug, Fault)]
/// enum DebugError {
///     #[fault("Debug value: {value:?}")]
///     Debug { value: Vec<u8> },
/// }
/// ```
#[proc_macro_derive(Fault, attributes(fault))]
pub fn derive_fault(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);

    let struct_or_enum_name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let format_body = match &input.data {
        syn::Data::Enum(data) => {
            let mut format_arms = Vec::new();

            for variant in &data.variants {
                let variant_name = &variant.ident;
                let mut custom_format = None;

                for attr in &variant.attrs {
                    if attr.path().is_ident("fault") {
                        if let Ok(format_str) = attr.parse_args::<syn::LitStr>() {
                            custom_format = Some(format_str.value());
                        }
                    }
                }

                let field_patterns: Vec<_> = variant
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, field)| {
                        let ident = field.ident.as_ref().map_or_else(
                            || {
                                let generated_ident = syn::Ident::new(
                                    &format!("__self_{}", i),
                                    proc_macro2::Span::call_site(),
                                );
                                quote! { #generated_ident }
                            },
                            |ident| quote! { #ident },
                        );
                        quote! { #ident }
                    })
                    .collect();

                let is_struct_variant = variant.fields.iter().any(|field| field.ident.is_some());
                let variant_pattern = if is_struct_variant {
                    let field_names: Vec<_> = variant
                        .fields
                        .iter()
                        .map(|field| {
                            field
                                .ident
                                .as_ref()
                                .map_or_else(|| quote! {}, |ident| quote! { #ident })
                        })
                        .collect();
                    quote! { { #(#field_names),* } }
                } else {
                    quote! { ( #(#field_patterns),* ) }
                };

                if let Some(format_str) = custom_format {
                    // Extract list of field names from format string
                    // {field_name} or {field_name:?}
                    let format_fields: Vec<_> = regex::Regex::new(r"\{(\w+)(:\?)?\}")
                        .unwrap()
                        .captures_iter(&format_str)
                        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
                        .collect();

                    // Generate list of field patterns
                    // __self_0, __self_1, etc., or field_name
                    let ordered_field_patterns: Vec<_> = format_fields
                        .iter()
                        .map(|field_name| {
                            if let Ok(index) = usize::from_str_radix(field_name, 10) {
                                field_patterns[index].clone()
                            } else {
                                let ident =
                                    syn::Ident::new(field_name, proc_macro2::Span::call_site());
                                quote! { #ident }
                            }
                        })
                        .collect();

                    // Replace {field_name} with {}
                    let modified_format_str = regex::Regex::new(r"\{(\w+)\}")
                        .unwrap()
                        .replace_all(&format_str, "{}")
                        .to_string();

                    // Replace {field_name:?} with {:?}
                    let modified_format_str = regex::Regex::new(r"\{(\w+):\?\}")
                        .unwrap()
                        .replace_all(&modified_format_str, "{:?}")
                        .to_string();

                    let write = backend_write(&modified_format_str, &ordered_field_patterns);
                    if field_patterns.len() > 0 {
                        format_arms.push(quote! {
                            #struct_or_enum_name::#variant_name #variant_pattern => #write,
                        });
                    } else {
                        format_arms.push(quote! {
                            #struct_or_enum_name::#variant_name => #write,
                        });
                    }
                } else {
                    let variant_name_str = variant_name.to_string();
                    let write = backend_write(&variant_name_str, &[]);
                    if field_patterns.len() > 0 {
                        format_arms.push(quote! {
                            #struct_or_enum_name::#variant_name #variant_pattern => #write,
                        });
                    } else {
                        format_arms.push(quote! {
                            #struct_or_enum_name::#variant_name => #write,
                        });
                    }
                }
            }

            quote! {
                match self {
                    #(#format_arms)*
                }
            }
        }
        syn::Data::Struct(_data) => {
            let mut custom_format = None;

            for attr in &input.attrs {
                if attr.path().is_ident("fault") {
                    if let Ok(format_str) = attr.parse_args::<syn::LitStr>() {
                        custom_format = Some(format_str.value());
                    }
                }
            }

            if let Some(format_str) = custom_format {
                // Extract list of field names from format string
                // {field_name} or {field_name:?}
                let format_fields: Vec<_> = regex::Regex::new(r"\{(\w+)(:\?)?\}")
                    .unwrap()
                    .captures_iter(&format_str)
                    .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
                    .collect();

                // Generate list of field patterns
                // self.field_name
                let field_patterns: Vec<_> = format_fields
                    .iter()
                    .map(|field_name| {
                        let generated_ident =
                            syn::Ident::new(field_name, proc_macro2::Span::call_site());
                        quote! { self.#generated_ident }
                    })
                    .collect();

                // Replace {field_name} with {}
                let modified_format_str = regex::Regex::new(r"\{(\w+)\}")
                    .unwrap()
                    .replace_all(&format_str, "{}")
                    .to_string();

                // Replace {field_name:?} with {:?}
                let modified_format_str = regex::Regex::new(r"\{(\w+):\?\}")
                    .unwrap()
                    .replace_all(&modified_format_str, "{:?}")
                    .to_string();

                backend_write(&modified_format_str, &field_patterns)
            } else {
                let name_str = struct_or_enum_name.to_string();
                backend_write(&name_str, &[])
            }
        }
        _ => unreachable!(),
    };

    let trait_impls = match BACKEND {
        Backend::Defmt => quote! {
            impl #impl_generics ::scars_fault::Fault for #struct_or_enum_name #ty_generics #where_clause {
                fn defmt_format(&self, __fmt: ::scars_fault::__defmt::Formatter<'_>) {
                    #format_body
                }
            }

            impl #impl_generics ::scars_fault::__defmt::Format for #struct_or_enum_name #ty_generics #where_clause {
                fn format(&self, __fmt: ::scars_fault::__defmt::Formatter<'_>) {
                    <Self as ::scars_fault::Fault>::defmt_format(self, __fmt)
                }
            }
        },
        Backend::Display => quote! {
            impl #impl_generics ::scars_fault::Fault for #struct_or_enum_name #ty_generics #where_clause {}

            impl #impl_generics ::core::fmt::Display for #struct_or_enum_name #ty_generics #where_clause {
                fn fmt(&self, __fmt: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    #format_body
                }
            }
        },
    };

    TokenStream::from(trait_impls)
}

fn backend_write(format_str: &str, args: &[proc_macro2::TokenStream]) -> proc_macro2::TokenStream {
    match BACKEND {
        Backend::Defmt => {
            if args.is_empty() {
                quote! { ::scars_fault::__defmt::write!(__fmt, #format_str) }
            } else {
                quote! { ::scars_fault::__defmt::write!(__fmt, #format_str, #(#args),*) }
            }
        }
        Backend::Display => {
            if args.is_empty() {
                quote! { ::core::write!(__fmt, #format_str) }
            } else {
                quote! { ::core::write!(__fmt, #format_str, #(#args),*) }
            }
        }
    }
}

/// Marks a function as the fault handler.
///
/// The function must have the signature `fn(&FaultInfo) -> !`. The macro
/// re-exports it under the symbol `_fault_handler`, overriding the weak
/// default provided by `scars-fault`.
///
/// # Examples
///
/// ```rust,ignore
/// use scars_fault::{Fault, FaultInfo, fault_handler};
///
/// #[fault_handler]
/// fn my_handler(info: &FaultInfo) -> ! {
///     if let Some(location) = info.location {
///         // Log error with location
///     }
///     loop {}
/// }
/// ```
#[proc_macro_attribute]
pub fn fault_handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::ItemFn);
    let fn_block = &input.block;
    let fn_attrs = &input.attrs;
    let fn_vis = &input.vis;
    let fn_sig = &input.sig;
    let fn_ident = &input.sig.ident;

    quote! {
        #(#fn_attrs)*
        #[unsafe(export_name = "_fault_handler")]
        #fn_vis #fn_sig {
            const _: () = {
                let _: fn(&::scars_fault::FaultInfo) -> ! = #fn_ident;
            };
            #fn_block
        }
    }
    .into()
}

/// Raises a fault by passing the given `Fault` value to the registered handler.
///
/// Takes an expression that evaluates to a value implementing `Fault`
/// and forwards it to the handler installed via `#[fault_handler]` (or the
/// weak default in `scars-fault`).
///
/// # Examples
///
/// ```rust,ignore
/// use scars_fault::{Fault, fault};
///
/// #[derive(Debug, Fault)]
/// #[fault("My error occurred")]
/// struct MyError;
///
/// fault!(MyError);
/// ```
#[proc_macro]
pub fn fault(input: TokenStream) -> TokenStream {
    let error = parse_macro_input!(input as syn::Expr);

    quote! {
        unsafe {
            ::scars_fault::handle_fault(&#error)
        }
    }
    .into()
}
