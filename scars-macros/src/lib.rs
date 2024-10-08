extern crate proc_macro;
use darling::ast::NestedMeta;
use darling::{Error, FromMeta};
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

#[derive(Debug, FromMeta)]
struct EntryArgs {
    name: String,
    priority: u8,
    stack_size: usize,
}

#[proc_macro_attribute]
pub fn entry(args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);
    let attr_args = match NestedMeta::parse_meta_list(args.into()) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(Error::from(e).write_errors());
        }
    };
    let args = match EntryArgs::from_list(&attr_args) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(e.write_errors());
        }
    };
    let thread_name = &args.name;
    let thread_priority = &args.priority;
    let thread_stack_size = &args.stack_size;
    let main_fn_ident = &item.sig.ident;
    quote! {
        #item
        #[export_name = "_start_main_thread"]
        pub fn _start_main_thread() {
            let main_thread = ::scars::make_thread!(#thread_name, ::scars::Priority::thread(#thread_priority), #thread_stack_size);
            main_thread.start(|| {
                #main_fn_ident();
            });
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_idle_thread_hook"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            #block
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_new"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
            #block
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_exec_begin"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
            #block
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_exec_end"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
            #block
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_ready_begin"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
            #block
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_thread_ready_end"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::ThreadRef) = #ident;
            #block
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_system_idle"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            #block
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_isr_enter"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            #block
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_isr_exit"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            #block
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
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_isr_exit_to_scheduler"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            #block
        }
    }
    .into()
}
