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

/// A request's raw, un-coerced parameter object. It crosses `Deserialize`
/// untouched so a param-bearing handler can pair it with the sibling field that
/// names its schema (a void/filter type id) and run the `ParamDef`-driven
/// coercion ([`DarklyEngine::coerce_void_params`]) — the one coercion a generic
/// macro can't perform, because it can't know which sibling field selects the
/// schema. `JsonValue` in the generated TS client.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(transparent)]
pub struct RawParams(pub Value);

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
}

impl Response {
    /// JSON-only response (the common case).
    pub fn json(value: Value) -> Self {
        Response { value, bytes: None }
    }

    /// JSON envelope + binary side-channel payload (always surfaces a `bytes`
    /// field downstream, even when `bytes` is empty).
    pub fn binary(value: Value, bytes: Vec<u8>) -> Self {
        Response {
            value,
            bytes: Some(bytes),
        }
    }

    /// A void mutation that returns nothing.
    pub fn empty() -> Self {
        Response {
            value: Value::Null,
            bytes: None,
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

/// Encode raw bytes onto the binary side-channel (JSON value `null`) — the
/// `#[handler(returns = bytes)]` target for engine methods that return
/// `Vec<u8>` or `[u8; N]` (thumbnails, previews, picked colors).
pub fn bytes_result<T: Into<Vec<u8>>>(bytes: T) -> Result<Response, ProtocolError> {
    Ok(Response::binary(Value::Null, bytes.into()))
}

// ---------------------------------------------------------------------------
// Response conversion — natural engine return types → `Response`
// ---------------------------------------------------------------------------
//
// The `#[handlers]` macro converts a method's return value with
// `(&result).response_kind().into_response(result)`. Two tag types disambiguate
// the otherwise-overlapping cases via autoref-based specialization (the trick
// Tauri's command macro uses, `tauri/src/ipc/command.rs`): the *general* impl is
// on `&T` and the *specialized* one on the value, so for a `Result` the value
// impl wins (it matches with one fewer autoref) and everything else falls
// through to the serialize path. `()` is `Serialize` → serializes to `null`,
// i.e. an empty response, with no special case.
//
// This keeps the engine methods returning their natural types (`()`, `LayerId`,
// `Result<LayerId, String>`, …) — clean for direct Rust callers — with no
// wire-encoding newtypes in their signatures.

/// Serialize path: any `T: Serialize`, including `()` → `null`. Reached via the
/// `&T` blanket impl, so a `Result` (which has its own value-level impl) is one
/// autoref closer and wins.
pub struct JsonResponseTag;

/// Reached for any `T: Serialize` through autoref. See [`JsonResponseTag`].
pub trait JsonResponseKind {
    #[inline(always)]
    fn response_kind(&self) -> JsonResponseTag {
        JsonResponseTag
    }
}
impl<T: serde::Serialize> JsonResponseKind for &T {}

impl JsonResponseTag {
    #[inline(always)]
    pub fn into_response<T: serde::Serialize>(self, value: T) -> Result<Response, ProtocolError> {
        let value =
            serde_json::to_value(value).map_err(|e| ProtocolError::Engine(e.to_string()))?;
        Ok(Response::json(value))
    }
}

/// Specialized path for `Result<T, String>`: `Ok` serializes; `Err` becomes a
/// [`ProtocolError::Engine`] rejection (the old `map_err(ProtocolError::engine)?`
/// convention). A handler that instead wants a recoverable `{ error }` *value*
/// (brush compile/validate) returns that shape explicitly rather than a
/// `Result`.
pub struct ResultResponseTag;

/// Reached only for `Result<T, String>` (value-level impl, wins the autoref
/// race against the `&T` serialize impl).
pub trait ResultResponseKind {
    #[inline(always)]
    fn response_kind(&self) -> ResultResponseTag {
        ResultResponseTag
    }
}
impl<T: serde::Serialize> ResultResponseKind for Result<T, String> {}

impl ResultResponseTag {
    #[inline(always)]
    pub fn into_response<T: serde::Serialize>(
        self,
        value: Result<T, String>,
    ) -> Result<Response, ProtocolError> {
        match value {
            Ok(v) => {
                let v =
                    serde_json::to_value(v).map_err(|e| ProtocolError::Engine(e.to_string()))?;
                Ok(Response::json(v))
            }
            Err(e) => Err(ProtocolError::Engine(e)),
        }
    }
}

/// The per-file unit a handler module returns from `register()`.
pub struct RequestRegistration {
    pub kind: &'static str,
    pub handle: fn(&mut DarklyEngine, Value, &[u8]) -> Result<Response, ProtocolError>,
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

// `macro_registrations()` — every `#[handler]`-tagged engine method's
// `RequestRegistration`, aggregated by `build.rs` (it scans `src/engine` for
// the attribute and emits one `DarklyEngine::__darkly_handler_<name>()` call
// per method). The `linkme` alternative doesn't compile on wasm32, so this
// build-time scan is the registration mechanism; it mirrors the `register()`
// file-scan the rest of the codebase uses.
include!(concat!(env!("OUT_DIR"), "/handler_registry_gen.rs"));

impl RequestRegistry {
    pub fn new() -> Self {
        let mut map: HashMap<&'static str, RequestRegistration> = HashMap::new();
        // Hand-written domain files plus the macro-generated registrations
        // coexist during the migration; the uniqueness assert below catches a
        // kind defined in both.
        for reg in handlers::registrations()
            .into_iter()
            .chain(macro_registrations())
        {
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
