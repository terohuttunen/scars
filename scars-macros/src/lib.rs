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
    let task_name = &args.name;
    let task_priority = &args.priority;
    let task_stack_size = &args.stack_size;
    let main_fn_ident = &item.sig.ident;
    quote! {
        #item
        #[export_name = "_start_main_task"]
        pub fn _start_main_task() {
            let main_task = ::scars::make_task!(#task_name, #task_priority, #task_stack_size);
            main_task.start(|| {
                #main_fn_ident();
            });
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn idle_task_hook(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_idle_task_hook"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn() = #ident;
            #block
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_task_new(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_task_new"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::TaskRef) = #ident;
            #block
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_task_exec_begin(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_task_exec_begin"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::TaskRef) = #ident;
            #block
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_task_exec_end(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_task_exec_end"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::TaskRef) = #ident;
            #block
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_task_ready_begin(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_task_ready_begin"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::TaskRef) = #ident;
            #block
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn trace_task_ready_end(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);

    let attrs = &item.attrs;
    let vis = &item.vis;
    let sig = &item.sig;
    let block = &item.block;

    let ident = &sig.ident;

    quote! {
        #[export_name = "_scars_trace_task_ready_end"]
        #(#attrs)* #vis #sig
        {
            let type_test: fn(::scars::TaskRef) = #ident;
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
