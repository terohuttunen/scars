extern crate proc_macro;
use darling;
use darling::FromMeta;
use darling::ast::NestedMeta;
use darling::util::parse_expr::preserve_str_literal;
use proc_macro::TokenStream;
use quote::{ToTokens, TokenStreamExt, format_ident, quote};
use syn::{
    AttrStyle, Attribute, ExprLit, Signature, Token, Visibility, braced, bracketed,
    parse::{Parse, ParseStream},
    parse_macro_input, token,
};

/// Function signature and body.
///
/// Same as `syn`'s `ItemFn` except we keep the body as a TokenStream instead of
/// parsing it. This makes the macro not error if there's a syntax error in the body,
/// which helps IDE autocomplete work better.
/// Adopted from Embassy.
#[derive(Debug, Clone)]
struct ItemFn {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub sig: Signature,
    pub brace_token: token::Brace,
    pub body: proc_macro2::TokenStream,
}

impl Parse for ItemFn {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut attrs = input.call(Attribute::parse_outer)?;
        let vis: Visibility = input.parse()?;
        let sig: Signature = input.parse()?;

        let content;
        let brace_token = braced!(content in input);
        while content.peek(Token![#]) && content.peek2(Token![!]) {
            let content2;
            attrs.push(Attribute {
                pound_token: content.parse()?,
                style: AttrStyle::Inner(content.parse()?),
                bracket_token: bracketed!(content2 in content),
                meta: content2.parse()?,
            });
        }

        let mut body = Vec::new();
        while !content.is_empty() {
            body.push(content.parse::<proc_macro2::TokenTree>()?);
        }
        let body = body.into_iter().collect();

        Ok(ItemFn {
            attrs,
            vis,
            sig,
            brace_token,
            body,
        })
    }
}

impl ToTokens for ItemFn {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        tokens.append_all(
            self.attrs
                .iter()
                .filter(|a| matches!(a.style, AttrStyle::Outer)),
        );
        self.vis.to_tokens(tokens);
        self.sig.to_tokens(tokens);
        self.brace_token.surround(tokens, |tokens| {
            tokens.append_all(self.body.clone());
        });
    }
}

#[derive(Debug, FromMeta)]
struct EntryArgs {
    #[darling(with = preserve_str_literal, map = "Some")]
    name: Option<syn::Expr>,
    priority: Option<syn::Expr>,
    stack_size: syn::Expr,
}

#[proc_macro_attribute]
pub fn entry(args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);
    let attr_args = match NestedMeta::parse_meta_list(args.into()) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(darling::Error::from(e).write_errors());
        }
    };
    let args = match EntryArgs::from_list(&attr_args) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(e.write_errors());
        }
    };
    // Thread name is the provided name, or the function name by default.
    let thread_name = args.name.clone().unwrap_or(syn::Expr::Lit(ExprLit {
        attrs: Vec::new(),
        lit: syn::Lit::Str(syn::LitStr::new(
            &item.sig.ident.to_string(),
            item.sig.ident.span(),
        )),
    }));
    let thread_priority = args.priority.clone().unwrap_or(syn::Expr::Lit(ExprLit {
        attrs: Vec::new(),
        lit: syn::Lit::Int(syn::LitInt::new("1", item.sig.ident.span())),
    }));
    let thread_stack_size = &args.stack_size;
    let main_fn_ident = &item.sig.ident;
    quote! {
        #item
        #[unsafe(export_name = "_start_main_thread")]
        pub fn _start_main_thread() {
               static MAIN_THREAD_STACK: ::scars::Stack<{#thread_stack_size}> = ::scars::Stack::new();
               static MAIN_THREAD: ::scars::Thread<{::scars::Priority::thread({#thread_priority})}, fn() -> !> = ::scars::Thread::new({#thread_name});
               MAIN_THREAD.init(MAIN_THREAD_STACK.init()).attach(#main_fn_ident).start();
        }
    }
    .into()
}

#[derive(Debug, FromMeta)]
struct TaskArgs {
    pool_size: syn::Expr,
}

#[proc_macro_attribute]
pub fn task(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(item as ItemFn);
    let attr_args = match NestedMeta::parse_meta_list(args.into()) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(darling::Error::from(e).write_errors());
        }
    };
    let args = match TaskArgs::from_list(&attr_args) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(e.write_errors());
        }
    };
    // Verify function signature:
    // async
    if item.sig.asyncness.is_none() {
        return syn::Error::new(item.sig.ident.span(), "task functions must be async")
            .to_compile_error()
            .into();
    }

    // generics
    if item.sig.generics.params.len() > 0 {
        return syn::Error::new(
            item.sig.ident.span(),
            "generic functions cannot be used as task functions",
        )
        .to_compile_error()
        .into();
    }

    // where-clause
    if item.sig.generics.where_clause.is_some() {
        return syn::Error::new(
            item.sig.ident.span(),
            "where clauses are not supported on task functions",
        )
        .to_compile_error()
        .into();
    }

    // abi
    if item.sig.abi.is_some() {
        return syn::Error::new(
            item.sig.ident.span(),
            "abi is not supported on task functions",
        )
        .to_compile_error()
        .into();
    }

    // variadic
    if item.sig.variadic.is_some() {
        return syn::Error::new(
            item.sig.ident.span(),
            "variadic functions cannot be used as task functions",
        )
        .to_compile_error()
        .into();
    }

    let return_type = match item.sig.output {
        syn::ReturnType::Default => quote! { ()},
        syn::ReturnType::Type(_, ref t) => {
            let ty = &*t;
            quote! { #ty }
        }
    };

    let pool_size = &args.pool_size;
    let fn_ident = item.sig.ident.clone();
    let vis = &item.vis;
    let inner_ident = format_ident!("__{}_interrupt_handler", fn_ident);
    item.sig.ident = inner_ident.clone();

    let mut fnarg_names = Vec::new();
    let mut outer_args = item.sig.inputs.clone();
    for arg in outer_args.iter_mut() {
        match arg {
            syn::FnArg::Receiver(_) => {}
            syn::FnArg::Typed(t) => match &mut *t.pat {
                syn::Pat::Ident(ident) => {
                    ident.mutability = None;
                    fnarg_names.push(ident.ident.clone());
                }
                _ => {}
            },
        }
    }

    quote! {

        #item

        #vis fn #fn_ident(#outer_args) -> Option<::scars::task::InitializedTask<#return_type>> {
            use ::scars::task::TaskPool;
            trait _TaskClosure {
                type F: ::core::future::Future<Output = #return_type>;
                fn define(#outer_args) -> Self::F;
            }

            impl _TaskClosure for () {
                type F = impl ::core::future::Future<Output = #return_type>;
                fn define(#outer_args) -> Self::F {
                    #inner_ident(#(#fnarg_names),*)
                }
            }
            static TASK_POOL: TaskPool<<() as _TaskClosure>::F, {#pool_size}> = TaskPool::new();
            TASK_POOL
                .alloc()
                .map(move |task| task.attach(move || <() as _TaskClosure>::define(#(#fnarg_names),*)))
        }

    }
    .into()
}

#[proc_macro_attribute]
pub fn idle_thread_hook(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[unsafe(export_name = "_scars_idle_thread_hook")]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            { #body }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_thread_new(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_new"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
            { #body }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_thread_exec_begin(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_exec_begin"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
             { #body }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_thread_exec_end(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_exec_end"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
            { #body }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_thread_ready_begin(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_ready_begin"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
            { #body }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_thread_ready_end(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_ready_end"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
            { #body }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_system_idle(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_system_idle"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            { #body }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_isr_enter(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_isr_enter"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            { #body }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_isr_exit(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_isr_exit"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            { #body }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_isr_exit_to_scheduler(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let body = &item.body;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_isr_exit_to_scheduler"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            { #body }
        }
    }
    .into()
}
