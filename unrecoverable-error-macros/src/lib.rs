//! Procedural macros for the `unrecoverable-error` crate.
//!
//! This crate provides procedural macros for the `unrecoverable-error` crate:
//! - `#[derive(UnrecoverableError)]`: Derives the `UnrecoverableError` trait with custom formatting
//! - `#[unrecoverable_error_handler]`: Marks a function as the error handler
//! - `unrecoverable_error!`: Macro for raising unrecoverable errors
//!
//! # Custom Error Formatting
//!
//! The derive macro supports custom formatting for both structs and enums:
//!
//! ```rust
//! use unrecoverable_error::UnrecoverableError;
//!
//! #[derive(Debug, UnrecoverableError)]
//! #[unrecoverable_error("Invalid configuration: {field} = {value}")]
//! struct ConfigError<'a> {
//!     field: &'a str,
//!     value: &'a str,
//! }
//!
//! #[derive(Debug, UnrecoverableError)]
//! enum MyError<'a> {
//!     #[unrecoverable_error("Invalid input: {value}")]
//!     InvalidInput { value: &'a str },
//!     #[unrecoverable_error("Timeout after {ms}ms")]
//!     Timeout { ms: u32 },
//!     #[unrecoverable_error("Connection failed: {reason}")]
//!     ConnectionFailed { reason: &'a str },
//! }
//! ```
//!
//! The format strings support both `{}` and `{:?}` for field values:
//!
//! ```rust
//! use unrecoverable_error::UnrecoverableError;
//!
//! #[derive(Debug, UnrecoverableError)]
//! enum DebugError {
//!     #[unrecoverable_error("Debug value: {value:?}")]
//!     Debug { value: Vec<u8> },
//! }
//! ```

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{ToTokens, quote};
use syn::parse_macro_input;

/// Derives the `UnrecoverableError` trait for a type with custom formatting.
///
/// This macro can be used on structs and enums to implement the `UnrecoverableError` trait.
/// It requires that the type already implements `Debug` and `Display`.
///
/// # Custom Formatting
///
/// You can customize the error message using the `#[unrecoverable_error]` attribute:
///
/// ```rust
/// use unrecoverable_error::UnrecoverableError;
///
/// #[derive(Debug, UnrecoverableError)]
/// #[unrecoverable_error("Invalid configuration: {field} = {value}")]
/// struct ConfigError<'a> {
///     field: &'a str,
///     value: &'a str,
/// }
/// ```
///
/// For enums, you can customize each variant:
///
/// ```rust
/// use unrecoverable_error::UnrecoverableError;
///
/// #[derive(Debug, UnrecoverableError)]
/// enum MyError<'a> {
///     #[unrecoverable_error("Invalid input: {value}")]
///     InvalidInput { value: &'a str },
///     #[unrecoverable_error("Timeout after {ms}ms")]
///     Timeout { ms: u32 },
/// }
/// ```
///
/// The format strings support both `{}` and `{:?}` for field values:
///
/// ```rust
/// use unrecoverable_error::UnrecoverableError;
///
/// #[derive(Debug, UnrecoverableError)]
/// enum DebugError {
///     #[unrecoverable_error("Debug value: {value:?}")]
///     Debug { value: Vec<u8> },
/// }
/// ```
///
/// # Examples
///
/// Basic usage:
///
/// ```rust
/// use unrecoverable_error::UnrecoverableError;
///
/// #[derive(Debug, UnrecoverableError)]
/// struct MyError;
/// ```
///
/// With custom formatting:
///
/// ```rust
/// use unrecoverable_error::UnrecoverableError;
///
/// #[derive(Debug, UnrecoverableError)]
/// #[unrecoverable_error("Error in {module}: {message}")]
/// struct ModuleError<'a> {
///     module: &'a str,
///     message: &'a str,
/// }
/// ```
///
/// Enum with custom formatting:
///
/// ```rust
/// use unrecoverable_error::UnrecoverableError;
///
/// #[derive(Debug, UnrecoverableError)]
/// enum NetworkError<'a> {
///     #[unrecoverable_error("Connection failed to {host}:{port}")]
///     ConnectionFailed { host: &'a str, port: u16 },
///     #[unrecoverable_error("Timeout after {ms}ms")]
///     Timeout { ms: u32 },
///     #[unrecoverable_error("Invalid response: {status:?}")]
///     InvalidResponse { status: Vec<u8> },
/// }
/// ```
#[proc_macro_derive(UnrecoverableError, attributes(unrecoverable_error))]
pub fn derive_unrecoverable_error(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);

    let struct_or_enum_name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let expanded = match &input.data {
        syn::Data::Enum(data) => {
            let mut format_arms = Vec::new();

            for variant in &data.variants {
                let variant_name = &variant.ident;
                let mut custom_format = None;

                for attr in &variant.attrs {
                    if attr.path().is_ident("unrecoverable_error") {
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
                        .map(|field| field.ident.as_ref().map_or_else(
                            || quote! {},
                            |ident| quote! { #ident },
                        ))
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
                                let ident = syn::Ident::new(field_name, proc_macro2::Span::call_site());
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

                    if field_patterns.len() > 0 {
                        format_arms.push(quote! {
                            #struct_or_enum_name::#variant_name #variant_pattern => write!(f, #modified_format_str, #(#ordered_field_patterns),*),
                        });
                    } else {
                        format_arms.push(quote! {
                            #struct_or_enum_name::#variant_name => write!(f, #modified_format_str),
                        });
                    }
                } else {
                    if field_patterns.len() > 0 {
                        format_arms.push(quote! {
                            #struct_or_enum_name::#variant_name #variant_pattern => write!(f, stringify!(#variant_name)),
                        });
                    } else {
                        format_arms.push(quote! {
                            #struct_or_enum_name::#variant_name => write!(f, stringify!(#variant_name)),
                        });
                    }
                }
            }

            quote! {
                impl #impl_generics UnrecoverableError for #struct_or_enum_name #ty_generics #where_clause {}

                impl #impl_generics core::fmt::Display for #struct_or_enum_name #ty_generics #where_clause {
                    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                        match self {
                            #(#format_arms)*
                        }
                    }
                }
            }
        }
        syn::Data::Struct(data) => {
            let mut custom_format = None;

            for attr in &input.attrs {
                if attr.path().is_ident("unrecoverable_error") {
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

                quote! {
                    impl #impl_generics UnrecoverableError for #struct_or_enum_name #ty_generics #where_clause {}

                    impl #impl_generics core::fmt::Display for #struct_or_enum_name #ty_generics #where_clause {
                        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                            write!(f, #modified_format_str, #(#field_patterns),*)
                        }
                    }
                }
            } else {
                quote! {
                    impl #impl_generics UnrecoverableError for #struct_or_enum_name #ty_generics #where_clause {}

                    impl #impl_generics core::fmt::Display for #struct_or_enum_name #ty_generics #where_clause {
                        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                            write!(f, stringify!(#struct_or_enum_name))
                        }
                    }
                }
            }
        }
        _ => unreachable!(),
    };

    TokenStream::from(expanded)
}

/// Marks a function as the unrecoverable error handler.
///
/// This attribute macro marks a function as the handler for unrecoverable errors.
/// The function must have the signature `fn(&UnrecoverableErrorInfo) -> !`.
///
/// # Examples
///
/// Using the fully qualified path:
///
/// ```rust
/// use unrecoverable_error::{UnrecoverableError, UnrecoverableErrorInfo};
///
/// #[unrecoverable_error_handler]
/// fn my_handler(info: &UnrecoverableErrorInfo) -> ! {
///     if let Some(location) = info.location {
///         // Log error with location
///     }
///     // Terminate the program
///     core::process::exit(1);
/// }
/// ```
///
/// Using the type directly:
///
/// ```rust
/// use unrecoverable_error::UnrecoverableError;
///
/// #[unrecoverable_error_handler]
/// fn my_handler(info: &::unrecoverable_error::UnrecoverableErrorInfo) -> ! {
///     if let Some(location) = info.location {
///         // Log error with location
///     }
///     // Terminate the program
///     core::process::exit(1);
/// }
/// ```
#[proc_macro_attribute]
pub fn unrecoverable_error_handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::ItemFn);
    let fn_block = &input.block;
    let fn_attrs = &input.attrs;
    let fn_vis = &input.vis;
    let fn_sig = &input.sig;
    let fn_ident = &input.sig.ident;

    quote! {
        #(#fn_attrs)*
        #[unsafe(export_name = "_unrecoverable_error_handler")]
        #fn_vis #fn_sig {
            const _: () = {
                let _: fn(&::unrecoverable_error::UnrecoverableErrorInfo) -> ! = #fn_ident;
            };
            #fn_block
        }
    }
    .into()
}

/// Raises an unrecoverable error.
///
/// This macro takes an expression that evaluates to an `UnrecoverableError`
/// and calls the error handler with it.
///
/// # Examples
///
/// ```rust
/// use unrecoverable_error::{UnrecoverableError, unrecoverable_error};
///
/// #[derive(Debug, UnrecoverableError)]
/// #[unrecoverable_error("My error occurred")]
/// struct MyError;
///
/// // This will call the error handler
/// unrecoverable_error!(MyError);
/// ```
#[proc_macro]
pub fn unrecoverable_error(input: TokenStream) -> TokenStream {
    let error = parse_macro_input!(input as syn::Expr);

    quote! {
        unsafe {
            unrecoverable_error::handle_unrecoverable_error(&#error)
        }
    }
    .into()
}
