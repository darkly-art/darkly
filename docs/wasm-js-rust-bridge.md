# WASM JS-Rust Bridge: Serialization Approaches

## Graphite's Approach: serde-wasm-bindgen

Graphite uses `serde-wasm-bindgen` to convert Rust structs directly to/from JS objects without an intermediate JSON string.

**Rust side** (`frontend/wasm/src/editor_api.rs`):
- `#[wasm_bindgen]` on ~75 functions in an `EditorHandle` struct
- `serde-wasm-bindgen::Serializer` with `.serialize_large_number_types_as_bigints(true)` for u64 → BigInt
- Custom proc macros (`#[impl_message]`, `ToDiscriminant`, `AsMessage`, `TransitiveChild`) for compile-time message routing
- `specta` derives for TS type generation (currently disabled)
- `ron` serialization for native/desktop CEF communication as a separate path

**JS side** (`frontend/src/messages.ts`, `subscription-router.ts`):
- `class-transformer` library (`plainToInstance`) to hydrate plain JS objects into typed TS class instances
- Runtime decorator-based — no compile-time guarantees if Rust message fields change

**Supporting stack:**
- `wasm-bindgen` — core FFI
- `serde` derives on all message types
- `js-sys` / `web-sys` for JS and DOM interop

## Alternative Approaches

### tsify

Uses serde-wasm-bindgen under the hood but also generates TypeScript type declarations automatically. Solves the type safety gap — you get `.d.ts` types for free. Probably the best general-purpose option today for structured messages.

### JSON string passing

`serde_json::to_string` on Rust side, `JSON.parse` on JS side. Simpler, surprisingly competitive for small payloads. Worse for large/complex objects due to string allocation + parse cost. Easy to debug (you can log the JSON).

### Shared memory / zero-copy

Best performance — no serialization at all for bulk data (image buffers, vertex arrays). Data is accessed as typed array views (`Uint8Array`, `Float32Array`) directly into WASM linear memory. Not practical for structured messages, only for homogeneous buffers. Graphite already uses this for image data.

### wasm-bindgen manual conversion

Manual `JsValue` conversion with `js_sys` types. Maximum control, very tedious. Only worth it for a handful of hot-path calls.

### Protocol Buffers / FlatBuffers

Schema-based serialization with codegen for both sides. Good for structured messages with versioning needs. Overkill for most WASM-in-browser scenarios.

## Recommendations by Data Type

| Data type | Best approach |
|---|---|
| Bulk pixel / geometry data | Shared memory views (zero-copy) |
| Structured messages / commands | `tsify` or `serde-wasm-bindgen` |
| Small, infrequent payloads | JSON string passing (simplest) |
| Hot-path with fixed schema | Manual `JsValue` conversion |

## Key Weakness in Graphite's Stack

The Rust→JS serialization is fine. The weakness is the JS side: `class-transformer` is a runtime decorator-based system with no compile-time guarantees. If a Rust message field changes, nothing in the TS build catches it. A tighter approach would be `tsify` or a codegen step that produces TS types from the Rust definitions.
