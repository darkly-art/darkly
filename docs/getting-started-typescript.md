# Getting Started: Darkly in TypeScript

This is the **page 1** guide to embedding Darkly's engine in a web app. It covers
the three things you need to do to put paint on a canvas: **import**,
**instantiate**, and **drive** the engine. Everything here lives in
`frontend/` and talks to the Rust core through the WASM bridge.

> Darkly is GPU-native: it renders with **WebGPU**. The page must be served over
> a secure context (`https://` or `localhost`) in a browser with WebGPU enabled.

## The shape of the API

You never call engine methods directly. Instead you talk to one object (an
`Engine`) that speaks a single async **request/response protocol**:

- `engine.send(kind, payload?, bytes?)` → a `Promise` that resolves with the
  response. Use it when you need the result.
- `engine.post(kind, payload?, bytes?)` → fire-and-forget. Use it for
  pointer-frequency mutations where you don't await anything.
- `engine.render(timeSecs)` → composites one frame and returns frame status.
  You call this from a `requestAnimationFrame` loop.

Requests are addressed by a typed `kind` string (`'add_raster'`,
`'begin_stroke'`, `'copy'`, …). The full list is the generated `RequestKind`
union in [`frontend/src/engine/protocol_gen.ts`](../frontend/src/engine/protocol_gen.ts).

Under the hood every request is enqueued onto one FIFO and drained either on a
`MessageChannel` macrotask (sub-frame, for `send`/`post`) or inside `render()`
(at frame time). You don't manage any of that; you just `await` your promise.

## 1. Import & instantiate

The bridge ships as the `darkly_wasm` package built from
[`frontend/wasm/`](../frontend/wasm/). Two pieces matter: the default `init()`
export (loads the `.wasm` module) and `DarklySession` (owns the GPU device).

The project wraps both in [`frontend/src/state/session.ts`](../frontend/src/state/session.ts),
which is the recommended entry point:

```ts
import init, { DarklySession } from '../wasm/pkg/darkly_wasm';
import { Engine } from './engine/protocol';

// 1. Load the WASM module once per process.
await init();

// 2. A session owns one wgpu device. Mint one per process and share it.
const session = new DarklySession();

// 3. Bind a handle to a <canvas>, then wrap it in the typed Engine transport.
const canvas = document.querySelector('canvas')!;
const handle = await session.createHandle(canvas, /* docWidth */ 1920, /* docHeight */ 1080);
const engine = new Engine(handle);
```

`session.createHandle(canvas, w, h)` allocates the WebGPU device on the first
call and **reuses it** for every later handle: that's how the multi-tab editor
runs N documents (N handles) on one device. `docWidth`/`docHeight` are the
document's pixel dimensions; the canvas's own CSS/backing size is independent.

In this repo the same flow is exposed as a one-call helper:

```ts
import { createHandle } from './state/session';

const engine = await createHandle(canvas, 1920, 1080); // returns an Engine
```

> **Single instance vs. shared device.** If you only ever have one canvas you
> can skip the session and call `DarklyHandle.create(canvas, w, h)` directly:
> it allocates its own device. Prefer `DarklySession` whenever more than one
> canvas is in play.

## 2. Drive the engine

### Mutate the document

```ts
// Add a raster layer and get its id back.
const { layerId } = await engine.send('add_raster', { name: 'Layer 1' });

// Fire-and-forget mutations (no result needed), ideal for pointer streams.
engine.post('begin_stroke', { layerId, x: 100, y: 120, pressure: 0.8 });
engine.post('stroke_to',    { x: 140, y: 160, pressure: 0.9 });
engine.post('end_stroke',   {});
```

`send` rejects with an `EngineError` (`{ kind, message }`) on a protocol or
handler failure, so wrap awaited calls in `try/catch` where it matters. `post`
routes rejections to `reportEngineError` (a `console.error`) so a failed
fire-and-forget request is logged, never silently swallowed.

### Binary payloads

Some requests carry raw bytes alongside the JSON payload (e.g. uploading pixel
data), and some responses come back with bytes attached. Pass a `Uint8Array` as
the third argument; on a binary response the resolved value carries a `bytes`
field:

```ts
const png = new Uint8Array(await file.arrayBuffer());
await engine.send('load_image', { layerId }, png);

const exported = await engine.send('export', { format: 'png' });
downloadBytes(exported.bytes); // Uint8Array
```

### Render loop

Darkly composites on demand. Run a `requestAnimationFrame` loop and call
`render`; it drains any pending requests under the frame's borrow, then paints:

```ts
function frame(nowMs: number) {
    const status = engine.render(nowMs / 1000);
    // status: { busy, needsMore, state?, results? }
    // Keep animating only while the engine says there's more to do.
    if (status.needsMore) requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
```

`status.state` is a synchronously-readable snapshot the UI mirrors
(`frameCount`, `thumbnailVersion`, `dirty`, `hasSelection`); read it instead of
issuing extra queries each frame. `status.busy` is `true` only when a re-entrant
render couldn't get the engine; when you see it, **don't** schedule another
frame: the outer render already owns the work.

## 3. Tear down

When a document closes, release the handle so its GPU resources are freed and any
in-flight deferred requests (copy / export / save) are rejected rather than left
hanging:

```ts
engine.free();
```

## Putting it together

```ts
import init, { DarklySession } from '../wasm/pkg/darkly_wasm';
import { Engine } from './engine/protocol';

await init();
const session = new DarklySession();
const handle = await session.createHandle(canvas, 1920, 1080);
const engine = new Engine(handle);

const { layerId } = await engine.send('add_raster', { name: 'Background' });
engine.post('begin_stroke', { layerId, x: 10, y: 10, pressure: 1 });
engine.post('end_stroke', {});

(function loop(t: number) {
    if (engine.render(t / 1000).needsMore) requestAnimationFrame(loop);
})(performance.now());

// later…
engine.free();
```

## Where to go next

- Request kinds: [`frontend/src/engine/protocol_gen.ts`](../frontend/src/engine/protocol_gen.ts) (generated).
- The transport in detail: [`frontend/src/engine/protocol.ts`](../frontend/src/engine/protocol.ts).
- Anything involving x/y coordinates: read [`docs/coordinate-systems.md`](coordinate-systems.md) first.
- Embedding the engine outside the browser (native / tests): [`docs/getting-started-rust.md`](getting-started-rust.md).
