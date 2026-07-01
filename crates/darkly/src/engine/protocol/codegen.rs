//! Typed TS client generator (`ts-export` only).
//!
//! Walks the [`RequestRegistry`](super::RequestRegistry) and emits
//! `frontend/src/engine/protocol_gen.ts`: the transitive `Req`/`Resp` type
//! declarations (via ts-rs), the `RequestKind` union, and the `EngineApi` +
//! `makeApi` surface the frontend calls instead of stringly-typed `send`/`post`.
//! ts-rs and this module are behind the `ts-export` feature, so neither reaches
//! the production / wasm build; only the `protocol` codegen test enables it.

use ts_rs::Config;

use super::{RequestMode, RequestRegistration, RequestRegistry, TsCollector, TsTyRef};

/// `set_layer_visible` → `setLayerVisible`.
pub fn camel_case(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// The TS type reference for a request payload, or `None` when the method takes
/// no JSON argument.
fn req_ref(reg: &RequestRegistration, cfg: &Config) -> Option<String> {
    match reg.ts.req.as_ref()? {
        TsTyRef::Named { name, .. } => Some(name(cfg)),
        TsTyRef::Literal(s) => Some((*s).to_string()),
        TsTyRef::Void => None,
    }
}

/// The TS type reference for a response value, folding in the binary
/// side-channel (`bytes_out`).
fn resp_ref(reg: &RequestRegistration, cfg: &Config) -> String {
    let base = match &reg.ts.resp {
        TsTyRef::Named { name, .. } => Some(name(cfg)),
        TsTyRef::Literal(s) => Some((*s).to_string()),
        TsTyRef::Void => None,
    };
    match (base, reg.ts.bytes_out) {
        (None, false) => "void".to_string(),
        (None, true) => "{ bytes: Uint8Array }".to_string(),
        (Some(b), false) => b,
        (Some(b), true) => format!("({b}) & {{ bytes: Uint8Array }}"),
    }
}

/// Regenerate the full `protocol_gen.ts` contents.
pub fn generate_protocol_ts() -> String {
    let reg = RequestRegistry::new();
    let cfg = Config::default().with_large_int("number");
    let regs = reg.sorted_registrations();

    // Transitive, deduped type declarations for every Req/Resp across all kinds.
    let mut collector = TsCollector::new(&cfg);
    for r in &regs {
        if let Some(TsTyRef::Named { collect, .. }) = &r.ts.req {
            collect(&mut collector);
        }
        if let TsTyRef::Named { collect, .. } = &r.ts.resp {
            collect(&mut collector);
        }
    }

    let mut out = String::new();
    out.push_str(
        "// @generated from RequestRegistry (ts-rs) — do not edit by hand.\n\
         // Regenerate: DARKLY_REGEN_TS=1 cargo test -p darkly --test protocol --features testing,ts-export\n\n",
    );
    out.push_str(
        "export type JsonValue =\n    \
         | string | number | boolean | null\n    \
         | JsonValue[]\n    \
         | { [key: string]: JsonValue };\n\n",
    );

    for decl in &collector.decls {
        let decl = decl.trim_end();
        out.push_str("export ");
        out.push_str(decl);
        if !decl.ends_with(';') {
            out.push(';');
        }
        out.push_str("\n\n");
    }

    // The `RequestKind` union + array (still the registry's canonical kind list
    // and the wasm `enqueue` arg type, even though app code no longer names it).
    out.push_str("export type RequestKind =\n");
    for r in &regs {
        out.push_str(&format!("    | '{}'\n", r.kind));
    }
    out.push_str("    ;\n\n");
    out.push_str("export const REQUEST_KINDS: readonly RequestKind[] = [\n");
    for r in &regs {
        out.push_str(&format!("    '{}',\n", r.kind));
    }
    out.push_str("] as const;\n\n");

    // The minimal transport the generated client closes over. `Engine` supplies
    // an adapter over its private request/postFF methods.
    out.push_str(
        "/** The request boundary the generated client closes over. `request`\n \
         *  awaits a typed response; `postFF` fires and forgets. */\n\
         export interface Transport {\n    \
         request(kind: RequestKind, payload?: object, bytes?: Uint8Array): Promise<any>;\n    \
         postFF(kind: RequestKind, payload?: object, bytes?: Uint8Array): void;\n\
         }\n\n",
    );

    // EngineApi interface.
    out.push_str("/** Typed, per-kind engine surface. */\nexport interface EngineApi {\n");
    for r in &regs {
        let method = camel_case(r.kind);
        let mut args: Vec<String> = Vec::new();
        if let Some(rt) = req_ref(r, &cfg) {
            args.push(format!("req: {rt}"));
        }
        if r.has_bytes_in {
            args.push("bytes: Uint8Array".to_string());
        }
        let args = args.join(", ");
        match r.mode {
            RequestMode::Send => {
                out.push_str(&format!(
                    "    {method}({args}): Promise<{}>;\n",
                    resp_ref(r, &cfg)
                ));
            }
            RequestMode::Post => {
                out.push_str(&format!("    {method}({args}): void;\n"));
            }
        }
    }
    out.push_str("}\n\n");

    // makeApi factory.
    out.push_str(
        "/** Build the typed client over a transport (in-process today, Tauri later). */\n\
         export function makeApi(t: Transport): EngineApi {\n    return {\n",
    );
    for r in &regs {
        let method = camel_case(r.kind);
        let has_req = req_ref(r, &cfg).is_some();
        let mut params: Vec<&str> = Vec::new();
        if has_req {
            params.push("req");
        }
        if r.has_bytes_in {
            params.push("bytes");
        }
        let params = params.join(", ");

        let mut call: Vec<String> = vec![format!("'{}'", r.kind)];
        if has_req {
            call.push("req".to_string());
            if r.has_bytes_in {
                call.push("bytes".to_string());
            }
        } else if r.has_bytes_in {
            call.push("{}".to_string());
            call.push("bytes".to_string());
        }
        let call = call.join(", ");
        let fwd = match r.mode {
            RequestMode::Send => "request",
            RequestMode::Post => "postFF",
        };
        out.push_str(&format!(
            "        {method}: ({params}) => t.{fwd}({call}),\n"
        ));
    }
    out.push_str("    };\n}\n");

    out
}
