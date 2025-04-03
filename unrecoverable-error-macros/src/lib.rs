extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{ToTokens, quote};
use syn::parse_macro_input;

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
