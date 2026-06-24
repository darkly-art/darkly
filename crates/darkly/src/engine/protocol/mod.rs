//! Async request/response protocol — the engine-side dispatch surface.
//!
//! This is the platform-agnostic core half of every transport (in-process /
//! Web Worker / Tauri): each backend deserializes a request to `(kind, payload,
//! bytes)`, calls [`RequestRegistry::dispatch`], and serializes the [`Response`]
//! back. There is **no central `match` on kind** — the registry is a registry,
//! not an enum, exactly like [`crate::gpu::veil`]. To add a request, drop one
//! file in `handlers/` exporting `pub fn register() -> RequestRegistration`;
//! `build.rs` discovers it (see [`build.rs`](../../../build.rs) `generate_registry`).
//!
//! ## Encoding rules
//! - Default both directions is `serde_json::Value` (ids, rects, params, layer
//!   tree, bools, floats).
//! - **Binary stays out of JSON** via the `bytes` side-channel on the request
//!   (`&[u8]` argument) and the [`Response`] (`bytes` field). Never base64.

use std::collections::HashMap;

use serde_json::Value;

use crate::engine::DarklyEngine;

pub mod handlers;
mod transport;

pub use transport::{DrainOutcome, QueuedRequest, RequestOutcome, Transport};

/// ParamDef-driven coercion of a JSON params object into `Vec<ParamValue>`.
/// Platform-agnostic — the protocol's replacement for the old JS-`Reflect`
/// `js_to_param_values` in the wasm bridge. Under the protocol, curve params
/// arrive as real JSON arrays (not JSON-encoded strings), which
/// [`param_values_from_json`](crate::gpu::params::param_values_from_json)
/// handles via `from_value`.
pub use crate::gpu::params::param_values_from_json as params_from_json;

/// A handler's successful result: a JSON value plus an optional binary
/// side-channel. `bytes` is `None` for non-binary requests and `Some` (possibly
/// empty) for binary ones — the `Some`/`None` distinction matters: a binary
/// request whose payload isn't ready yet (e.g. a preview readback in flight)
/// returns `Some(empty)`, which must still surface a `bytes` field to the caller
/// rather than collapsing to "no value". Repacked out-of-band by the transport
/// (zero-copy `Uint8Array` in the browser).
#[derive(Debug)]
pub struct Response {
    pub value: Value,
    pub bytes: Option<Vec<u8>>,
    /// When `true`, the handler kicked an async GPU readback instead of
    /// producing a result inline: the transport emits **no** outcome for the
    /// request this drain, and the engine pushes the request's terminal
    /// outcome onto its `completed_requests` queue once the readback lands (or
    /// fails). The `value`/`bytes` of a deferred response are ignored. Used by
    /// the one-shot readback ops (copy / cut / export / save) so the request
    /// that kicked the readback is the request that resolves with its result —
    /// no separate `poll_*` round-trip.
    pub deferred: bool,
}

impl Response {
    /// JSON-only response (the common case).
    pub fn json(value: Value) -> Self {
        Response {
            value,
            bytes: None,
            deferred: false,
        }
    }

    /// JSON envelope + binary side-channel payload (always surfaces a `bytes`
    /// field downstream, even when `bytes` is empty).
    pub fn binary(value: Value, bytes: Vec<u8>) -> Self {
        Response {
            value,
            bytes: Some(bytes),
            deferred: false,
        }
    }

    /// A void mutation that returns nothing.
    pub fn empty() -> Self {
        Response {
            value: Value::Null,
            bytes: None,
            deferred: false,
        }
    }

    /// A deferred response: the handler kicked an async readback and the
    /// request's promise stays pending until the engine resolves it via
    /// `completed_requests`. See [`Response::deferred`].
    pub fn deferred() -> Self {
        Response {
            value: Value::Null,
            bytes: None,
            deferred: true,
        }
    }
}

/// Protocol-level failure, distinct from a handler's *domain* result (a
/// recoverable `{ error }` encoded into the [`Response`] value). This rejects
/// the JS promise; the envelope `{ kind, message }` is what the TS side sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// No handler registered for the request kind (typo'd / stale frontend).
    UnknownRequest(String),
    /// Payload failed to deserialize into the handler's `Req` struct.
    BadPayload(String),
    /// The handler ran but the engine reported a structured failure that the
    /// handler chose to surface as a rejection rather than a `{ error }` value.
    Engine(String),
}

impl ProtocolError {
    pub fn unknown(kind: &str) -> Self {
        ProtocolError::UnknownRequest(kind.to_string())
    }

    pub fn engine(msg: impl Into<String>) -> Self {
        ProtocolError::Engine(msg.into())
    }

    /// `{ kind, message }` envelope for the transport to reject the promise with.
    pub fn to_json(&self) -> Value {
        let (kind, message) = match self {
            ProtocolError::UnknownRequest(k) => ("unknown_request", k.clone()),
            ProtocolError::BadPayload(m) => ("bad_payload", m.clone()),
            ProtocolError::Engine(m) => ("engine_error", m.clone()),
        };
        serde_json::json!({ "kind": kind, "message": message })
    }
}

/// Map a serde decode error onto a [`ProtocolError::BadPayload`].
pub fn bad_payload(e: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::BadPayload(e.to_string())
}

/// Decode a request payload into a handler's `Req` struct, mapping serde
/// failures onto [`ProtocolError::BadPayload`].
pub fn decode<T: serde::de::DeserializeOwned>(payload: Value) -> Result<T, ProtocolError> {
    serde_json::from_value(payload).map_err(bad_payload)
}

/// Decode a `{ id: u64 }` payload to a [`LayerId`] — the single most common
/// handler shape.
pub fn layer_id(payload: Value) -> Result<crate::layer::LayerId, ProtocolError> {
    #[derive(serde::Deserialize)]
    struct Id {
        id: u64,
    }
    let r: Id = decode(payload)?;
    Ok(crate::layer::LayerId::from_ffi(r.id))
}

/// Encode a node-graph mutation result. The engine returns the serialized graph
/// JSON on success; handlers resolve with `{ graph }` (parsed) or `{ error }` —
/// the brush-builder consumes both without throwing (matches the old
/// `graph_result` JsValue shape).
pub fn graph_result(r: Result<String, String>) -> Result<Response, ProtocolError> {
    match r {
        Ok(json) => {
            let graph: Value = serde_json::from_str(&json).map_err(bad_payload)?;
            Ok(Response::json(serde_json::json!({ "graph": graph })))
        }
        Err(e) => Ok(Response::json(serde_json::json!({ "error": e }))),
    }
}

/// Encode a `Result<(), String>` as either `null` (success) or `{ error }` —
/// the old `JsValue::NULL | from_str(e)` convention for brush compile/validate.
pub fn ok_or_error(r: Result<(), String>) -> Response {
    match r {
        Ok(()) => Response::json(Value::Null),
        Err(e) => Response::json(serde_json::json!({ "error": e })),
    }
}

/// A request handler's type-erased dispatch fn: decode the JSON payload (and
/// optional binary side-channel), mutate/query the engine, encode a [`Response`].
pub type HandleFn = fn(&mut DarklyEngine, Value, &[u8]) -> Result<Response, ProtocolError>;

/// Bound on a request's `Req` / `Resp` types. Under `ts-export` it requires
/// [`ts_rs::TS`] so the generator can emit the type's TS declaration; without the
/// feature it is a no-op blanket bound so release builds carry zero ts-rs code.
#[cfg(feature = "ts-export")]
pub trait WireType: ts_rs::TS + 'static {}
#[cfg(feature = "ts-export")]
impl<T: ts_rs::TS + 'static + ?Sized> WireType for T {}

#[cfg(not(feature = "ts-export"))]
pub trait WireType {}
#[cfg(not(feature = "ts-export"))]
impl<T: ?Sized> WireType for T {}

/// The per-kind unit a handler module registers. Beyond the dispatch fn it
/// carries — under `ts-export` — the request/response *type identity* the
/// generator pairs into a typed `EngineApi` method (`kind → (Req, Resp)`).
/// Construct via [`RequestRegistration::new`] / [`binary_in`](Self::binary_in) /
/// [`binary_out`](Self::binary_out) rather than a struct literal so the type
/// parameters (and thus the TS types) are always recorded.
pub struct RequestRegistration {
    pub kind: &'static str,
    pub handle: HandleFn,
    /// TS type identity for codegen. Present only under `ts-export`.
    #[cfg(feature = "ts-export")]
    pub ts: ts_export::TsSig,
}

impl RequestRegistration {
    /// JSON payload in, JSON (or void, `Resp = ()`) value out — the common case.
    pub fn new<Req: WireType, Resp: WireType>(kind: &'static str, handle: HandleFn) -> Self {
        Self::build::<Req, Resp>(kind, handle, false, false)
    }

    /// The request carries a binary side-channel payload (a `bytes: Uint8Array`
    /// argument), e.g. `paste_image` / `open_document` / `brush_import`.
    pub fn binary_in<Req: WireType, Resp: WireType>(kind: &'static str, handle: HandleFn) -> Self {
        Self::build::<Req, Resp>(kind, handle, true, false)
    }

    /// The response carries a binary side-channel payload (a `bytes: Uint8Array`
    /// field on the resolved value), e.g. `pick_color` / `poll_preview`.
    pub fn binary_out<Req: WireType, Resp: WireType>(kind: &'static str, handle: HandleFn) -> Self {
        Self::build::<Req, Resp>(kind, handle, false, true)
    }

    fn build<Req: WireType, Resp: WireType>(
        kind: &'static str,
        handle: HandleFn,
        req_bytes: bool,
        resp_bytes: bool,
    ) -> Self {
        // `req_bytes` / `resp_bytes` and the `Req` / `Resp` type params feed the
        // TS generator only; without `ts-export` they are intentionally unused.
        let _ = (req_bytes, resp_bytes);
        RequestRegistration {
            kind,
            handle,
            #[cfg(feature = "ts-export")]
            ts: ts_export::TsSig::of::<Req, Resp>(req_bytes, resp_bytes),
        }
    }
}

/// Auto-discovered request registry. The single generic dispatch surface; the
/// substitute for an enum's exhaustiveness check is the uniqueness assertion in
/// [`RequestRegistry::new`].
pub struct RequestRegistry {
    handlers: HashMap<&'static str, RequestRegistration>,
}

impl Default for RequestRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestRegistry {
    pub fn new() -> Self {
        let mut map: HashMap<&'static str, RequestRegistration> = HashMap::new();
        for reg in handlers::registrations() {
            let kind = reg.kind;
            let prev = map.insert(kind, reg);
            assert!(prev.is_none(), "duplicate request kind: {kind}");
        }
        RequestRegistry { handlers: map }
    }

    /// Every registered kind, sorted — feeds the generated TS `RequestKind`
    /// union and the "every kind is reachable" test.
    pub fn all_kinds(&self) -> Vec<&'static str> {
        let mut kinds: Vec<&'static str> = self.handlers.keys().copied().collect();
        kinds.sort_unstable();
        kinds
    }

    /// Each kind paired with its TS type signature, sorted by kind — the input
    /// to the typed-client generator. Present only under `ts-export`.
    #[cfg(feature = "ts-export")]
    pub fn ts_signatures(&self) -> Vec<(&'static str, &ts_export::TsSig)> {
        let mut sigs: Vec<(&'static str, &ts_export::TsSig)> =
            self.handlers.iter().map(|(k, reg)| (*k, &reg.ts)).collect();
        sigs.sort_by_key(|(k, _)| *k);
        sigs
    }

    /// The single generic entry point. A 2-arm match on `Option`, never on kind.
    pub fn dispatch(
        &self,
        engine: &mut DarklyEngine,
        kind: &str,
        payload: Value,
        bytes: &[u8],
    ) -> Result<Response, ProtocolError> {
        match self.handlers.get(kind) {
            Some(reg) => (reg.handle)(engine, payload, bytes),
            None => Err(ProtocolError::unknown(kind)),
        }
    }
}

/// TS type-identity capture for the typed-client generator. Compiled only under
/// `ts-export` (implied by `testing`); release / WASM builds never see ts-rs.
///
/// Each [`RequestRegistration`] records, via [`TsSig::of`], its request and
/// response Rust types. The generator ([`tests/protocol.rs`]) walks every
/// registration's `TsSig`, collects the transitive closure of named TS
/// declarations they reach ([`TsCollector`]), and emits both the interfaces and
/// a typed `EngineApi` that maps each `kind` to a method.
#[cfg(feature = "ts-export")]
pub mod ts_export {
    use std::any::TypeId;
    use std::collections::{BTreeMap, HashSet};

    use ts_rs::{Config, TS};

    /// Per-kind TS type identity captured at registration time.
    pub struct TsSig {
        /// The request payload type.
        pub req: TsRef,
        /// The response value type (`()` for a void mutation).
        pub resp: TsRef,
        /// The request carries a binary side-channel payload (`bytes` argument).
        pub req_bytes: bool,
        /// The response carries a binary side-channel payload (`bytes` field).
        pub resp_bytes: bool,
    }

    impl TsSig {
        pub fn of<Req: TS + 'static, Resp: TS + 'static>(
            req_bytes: bool,
            resp_bytes: bool,
        ) -> Self {
            TsSig {
                req: TsRef::of::<Req>(),
                resp: TsRef::of::<Resp>(),
                req_bytes,
                resp_bytes,
            }
        }
    }

    /// The [`Config`] every name/decl in the generated client resolves under.
    /// `TS_RS_LARGE_INT=number` maps `u64`/`i64` to TS `number` rather than the
    /// `bigint` default: every id/size in this protocol crosses the wasm
    /// boundary as a JS `number` (values stay ≤ 2^53 — see [`crate::engine::EngineState`]),
    /// so `bigint` would be wrong at every call site.
    pub fn config() -> Config {
        std::env::set_var("TS_RS_LARGE_INT", "number");
        Config::from_env()
    }

    /// A reference to one wire type: a fn resolving the TS type *name* under a
    /// given [`Config`] (e.g. `"CopyReq"`, `"JsonValue"`, `"null"`) plus a fn
    /// that records the type's transitive named declarations into a
    /// [`TsCollector`].
    pub struct TsRef {
        pub name: fn(&Config) -> String,
        pub collect: fn(&mut TsCollector),
    }

    impl TsRef {
        fn of<T: TS + 'static>() -> Self {
            TsRef {
                name: |cfg| T::name(cfg),
                collect: |c| c.visit_root::<T>(),
            }
        }
    }

    /// Accumulates the transitive closure of named TS declarations reachable
    /// from the registered request/response types — deduped by `TypeId`, keyed
    /// (and thus ordered) by type name for deterministic codegen.
    pub struct TsCollector {
        decls: BTreeMap<String, String>,
        seen: HashSet<TypeId>,
        cfg: Config,
    }

    impl Default for TsCollector {
        fn default() -> Self {
            TsCollector {
                decls: BTreeMap::new(),
                seen: HashSet::new(),
                cfg: config(),
            }
        }
    }

    impl TsCollector {
        /// Seed the walk from a top-level request/response type.
        pub fn visit_root<T: TS + 'static>(&mut self) {
            <Self as ts_rs::TypeVisitor>::visit::<T>(self);
        }

        /// Every collected declaration (`export type X = …;`), name-ordered.
        pub fn declarations(&self) -> impl Iterator<Item = &str> {
            self.decls.values().map(String::as_str)
        }
    }

    impl ts_rs::TypeVisitor for TsCollector {
        fn visit<T: TS + 'static + ?Sized>(&mut self) {
            if !self.seen.insert(TypeId::of::<T>()) {
                return;
            }
            // Only named, exportable types (those `#[derive(TS)]` assigns an
            // output path) get a declaration; primitives, containers, and
            // `serde_json::Value` (-> `JsonValue`) have none and are referenced
            // inline by `name`.
            if T::output_path().is_some() {
                // ts-rs renders some builtin decls (e.g. `JsonValue`) with a
                // trailing `;`; normalize so the `export …;` wrap is single.
                let decl = T::decl(&self.cfg);
                let decl = decl.trim_end().trim_end_matches(';');
                self.decls
                    .insert(T::name(&self.cfg), format!("export {decl};"));
            }
            T::visit_dependencies(self);
        }
    }
}
