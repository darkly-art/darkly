//! `darkly-macros` — the `#[handlers]` engine-bridge macro.
//!
//! Tag an `impl DarklyEngine { … }` block with `#[handlers]` and mark the
//! methods that should be reachable over the request/response protocol with an
//! inner `#[handler]` (or `#[handler(returns = graph)]`). For each marked
//! method the macro derives — from the signature alone — everything the old
//! hand-written `handlers/*.rs` files spelled out:
//!
//! - a `#[derive(Deserialize)]` `…Req` struct, one field per parameter (the
//!   receiver is dropped; a `bytes: &[u8]` parameter binds to the protocol's
//!   binary side-channel instead of a JSON field; `&str`/`&[T]` parameters
//!   become owned `String`/`Vec<T>` fields and are re-borrowed at the call);
//! - a `DarklyEngine::__darkly_handler_<name>()` associated fn returning the
//!   `RequestRegistration`, which decodes the `Req`, calls the method, and
//!   converts the return through the autoref-specialized response tags
//!   (`()`/`T: Serialize` → JSON, `Result<T, String>` → JSON or engine error).
//!
//! The kind string is the method name verbatim — the signature is the single
//! source of truth. `build.rs` scans for `#[handler]` and aggregates every
//! generated registration (no `linkme`: it doesn't compile on wasm32).
//!
//! Why an impl-block attribute rather than a per-method one: an attribute macro
//! on a method may only emit *associated* items, but the `Req` structs must be
//! top-level (nameable, `TS`-derivable). Wrapping the block lets one expansion
//! emit the cleaned impl, the top-level `Req` structs, and the registration
//! impl together.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, spanned::Spanned, Error, FnArg, Ident, ImplItem, ItemImpl, Pat, Type,
};

/// Attribute for `impl DarklyEngine` blocks. See the module docs.
#[proc_macro_attribute]
pub fn handlers(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemImpl);

    let mut req_structs: Vec<TokenStream2> = Vec::new();
    let mut reg_fns: Vec<TokenStream2> = Vec::new();

    for impl_item in &mut input.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let Some(attr_idx) = method
            .attrs
            .iter()
            .position(|a| a.path().is_ident("handler"))
        else {
            continue;
        };
        let attr = method.attrs.remove(attr_idx);
        let args = match parse_handler_args(&attr) {
            Ok(v) => v,
            Err(e) => return e.to_compile_error().into(),
        };
        match build_handler(method, args) {
            Ok(Built { req_struct, reg_fn }) => {
                if let Some(req) = req_struct {
                    req_structs.push(req);
                }
                reg_fns.push(reg_fn);
            }
            Err(e) => return e.to_compile_error().into(),
        }
    }

    let self_ty = &input.self_ty;
    quote! {
        #input

        #(#req_structs)*

        impl #self_ty {
            #(#reg_fns)*
        }
    }
    .into()
}

struct Built {
    /// `None` for a method whose only inputs are the receiver and `bytes`.
    req_struct: Option<TokenStream2>,
    reg_fn: TokenStream2,
}

/// How a handler's return value is shaped onto the wire. The default covers
/// the autoref-specialized `()` / `T: Serialize` / `Result<T, String>` cases;
/// the others are one-token `#[handler(returns = …)]` opt-ins for the response
/// shapes the engine's natural return type can't disambiguate on its own.
#[derive(Clone, Copy, PartialEq)]
enum ReturnMode {
    /// `()` → `null`, `T: Serialize` → JSON, `Result<T, String>::Err` → reject.
    Default,
    /// `Result<String, String>` recompiled-graph JSON → `{ graph } | { error }`.
    Graph,
    /// `Vec<u8>` / `[u8; N]` → the binary side-channel (JSON value is `null`).
    Bytes,
    /// `Result<(), String>` → `null | { error }` as a *value* (not a reject).
    OkError,
}

/// Parsed `#[handler(...)]` options.
struct HandlerArgs {
    returns: ReturnMode,
    /// Explicit `send`/`post` override; `None` derives the mode from the return
    /// type (`()` → `Post`, else `Send`).
    mode: Option<bool>, // Some(true) = send, Some(false) = post
}

/// Parse the optional `(returns = graph | bytes | ok_error, send | post)` body
/// of a `#[handler]` attribute. Bare `#[handler]` → defaults.
fn parse_handler_args(attr: &syn::Attribute) -> syn::Result<HandlerArgs> {
    let mut out = HandlerArgs {
        returns: ReturnMode::Default,
        mode: None,
    };
    match &attr.meta {
        syn::Meta::Path(_) => Ok(out),
        syn::Meta::List(_) => {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("returns") {
                    let value = meta.value()?;
                    let ident: Ident = value.parse()?;
                    out.returns =
                        match ident.to_string().as_str() {
                            "graph" => ReturnMode::Graph,
                            "bytes" => ReturnMode::Bytes,
                            "ok_error" => ReturnMode::OkError,
                            _ => return Err(meta.error(
                                "unknown `returns` mode (expected `graph`, `bytes`, or `ok_error`)",
                            )),
                        };
                    Ok(())
                } else if meta.path.is_ident("send") {
                    out.mode = Some(true);
                    Ok(())
                } else if meta.path.is_ident("post") {
                    out.mode = Some(false);
                    Ok(())
                } else {
                    Err(meta.error("unknown `#[handler]` option"))
                }
            })?;
            Ok(out)
        }
        syn::Meta::NameValue(_) => Err(Error::new(
            attr.span(),
            "`#[handler]` takes no `= value` form",
        )),
    }
}

/// If `ty` is `Result<T, _>`, return `T` — the success type the response
/// metadata (and the `Send` `Promise<Resp>`) is shaped from.
fn result_ok_type(ty: &Type) -> Option<&Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != "Result" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

/// Whether the method returns `()` (or has no return type).
fn returns_unit(ret: &syn::ReturnType) -> bool {
    match ret {
        syn::ReturnType::Default => true,
        syn::ReturnType::Type(_, ty) => matches!(&**ty, Type::Tuple(t) if t.elems.is_empty()),
    }
}

/// One decoded parameter of a handler method.
struct Param {
    /// `req.<field>` access (owned) or `&req.<field>` (re-borrowed) or the raw
    /// `bytes` side-channel — the expression passed at the call site.
    call_expr: TokenStream2,
    /// `Some((field_ident, field_ty))` for a JSON `Req` field; `None` for the
    /// `bytes` side-channel parameter.
    field: Option<(Ident, TokenStream2)>,
}

fn build_handler(method: &syn::ImplItemFn, args: HandlerArgs) -> syn::Result<Built> {
    let returns = args.returns;
    let name = &method.sig.ident;
    let kind = name.to_string();

    let mut params: Vec<Param> = Vec::new();
    for input in &method.sig.inputs {
        let FnArg::Typed(pat_ty) = input else {
            continue; // receiver (`&self` / `&mut self`)
        };
        let Pat::Ident(pat_ident) = &*pat_ty.pat else {
            return Err(Error::new(
                pat_ty.pat.span(),
                "#[handler] parameters must be plain identifiers",
            ));
        };
        let ident = pat_ident.ident.clone();
        params.push(parse_param(ident, &pat_ty.ty)?);
    }

    let fields: Vec<&(Ident, TokenStream2)> =
        params.iter().filter_map(|p| p.field.as_ref()).collect();

    // The `Req` struct (omitted when the method takes only `bytes`).
    let req_ident = format_ident!("{}Req", pascal_case(&kind));
    let req_struct = if fields.is_empty() {
        None
    } else {
        let field_defs = fields.iter().map(|(id, ty)| quote!(#id: #ty));
        Some(quote! {
            #[derive(serde::Deserialize)]
            #[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
            pub(crate) struct #req_ident {
                #(#field_defs,)*
            }
        })
    };

    // Closure parameter names: underscore the ones a given handler doesn't read,
    // so the generated `fn` pointer never trips the unused-variable lint.
    let payload_ident = if fields.is_empty() {
        format_ident!("_payload")
    } else {
        format_ident!("payload")
    };
    let uses_bytes = params.iter().any(|p| p.field.is_none());
    let bytes_ident = if uses_bytes {
        format_ident!("bytes")
    } else {
        format_ident!("_bytes")
    };

    let decode = if fields.is_empty() {
        quote!()
    } else {
        quote!(let req: #req_ident = crate::engine::protocol::decode(payload)?;)
    };

    let call_args = params.iter().map(|p| &p.call_expr);
    let call = quote!(let __result = engine.#name(#(#call_args),*););

    let convert = match returns {
        ReturnMode::Graph => quote!(crate::engine::protocol::graph_result(__result)),
        ReturnMode::Bytes => quote!(crate::engine::protocol::bytes_result(__result)),
        ReturnMode::OkError => quote!(Ok(crate::engine::protocol::ok_or_error(__result))),
        ReturnMode::Default => quote! {{
            use crate::engine::protocol::{JsonResponseKind as _, ResultResponseKind as _};
            let __tag = (&__result).response_kind();
            __tag.into_response(__result)
        }},
    };

    // Codegen metadata + transport mode, derived from the signature and the
    // `returns`/`send`/`post` opts. The builder setters are no-ops without the
    // `ts-export` feature, so they're always emitted unconditionally.
    let req_call = if fields.is_empty() {
        quote!()
    } else {
        quote!(.req::<#req_ident>())
    };
    let bytes_in_call = if uses_bytes {
        quote!(.bytes_in())
    } else {
        quote!()
    };

    // The response type reference, and the default mode for the return type.
    let ret_ty = &method.sig.output;
    let is_unit = returns_unit(ret_ty);
    let resp_call = match returns {
        ReturnMode::Graph => quote!(.resp_literal("{ graph: JsonValue } | { error: string }")),
        ReturnMode::OkError => quote!(.resp_literal("null | { error: string }")),
        ReturnMode::Bytes => quote!(.bytes_out()),
        ReturnMode::Default => {
            if is_unit {
                quote!()
            } else if let syn::ReturnType::Type(_, ty) = ret_ty {
                let resp_ty = result_ok_type(ty).unwrap_or(ty);
                quote!(.resp::<#resp_ty>())
            } else {
                quote!()
            }
        }
    };

    // Mode: explicit `send`/`post` wins; otherwise `()` + `Default` → `Post`.
    let default_send = !(is_unit && returns == ReturnMode::Default);
    let send = args.mode.unwrap_or(default_send);
    let mode_call = if send {
        quote!(.send())
    } else {
        quote!(.post())
    };

    let reg_fn_ident = format_ident!("__darkly_handler_{}", name);
    let reg_fn = quote! {
        #[doc(hidden)]
        pub(crate) fn #reg_fn_ident() -> crate::engine::protocol::RequestRegistration {
            crate::engine::protocol::RequestRegistration::new(
                #kind,
                |engine, #payload_ident, #bytes_ident| {
                    #decode
                    #call
                    #convert
                },
            )
            #mode_call
            #bytes_in_call
            #req_call
            #resp_call
        }
    };

    Ok(Built { req_struct, reg_fn })
}

/// Classify one parameter: side-channel `bytes`, an owned field, or a borrowed
/// field whose owned form (`String`/`Vec<T>`) is what the `Req` carries.
fn parse_param(ident: Ident, ty: &Type) -> syn::Result<Param> {
    // The binary side-channel: `bytes: &[u8]`.
    if ident == "bytes" {
        if let Type::Reference(r) = ty {
            if let Type::Slice(slice) = &*r.elem {
                if is_u8(&slice.elem) {
                    return Ok(Param {
                        call_expr: quote!(bytes),
                        field: None,
                    });
                }
            }
        }
        return Err(Error::new(
            ty.span(),
            "a `bytes` handler parameter must be `&[u8]` (the protocol side-channel)",
        ));
    }

    match ty {
        // `&str` → owned `String` field, re-borrowed at the call.
        Type::Reference(r) if is_str(&r.elem) => Ok(Param {
            call_expr: quote!(&req.#ident),
            field: Some((ident.clone(), quote!(String))),
        }),
        // `&[T]` → owned `Vec<T>` field, re-borrowed at the call.
        Type::Reference(r) => {
            if let Type::Slice(slice) = &*r.elem {
                let elem = &slice.elem;
                Ok(Param {
                    call_expr: quote!(&req.#ident),
                    field: Some((ident.clone(), quote!(Vec<#elem>))),
                })
            } else {
                Err(Error::new(
                    ty.span(),
                    "unsupported reference parameter (only `&str`, `&[T]`, and `bytes: &[u8]` are handled)",
                ))
            }
        }
        // Owned parameter — the `Req` field is the type verbatim.
        _ => Ok(Param {
            call_expr: quote!(req.#ident),
            field: Some((ident.clone(), quote!(#ty))),
        }),
    }
}

fn is_u8(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.is_ident("u8"))
}

fn is_str(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.is_ident("str"))
}

/// `add_group` → `AddGroup`, `move_layers` → `MoveLayers`.
fn pascal_case(snake: &str) -> String {
    snake
        .split('_')
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}
