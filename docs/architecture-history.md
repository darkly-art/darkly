# Architecture History: the engine boundary, and how it got this way

> **If you are here because you want to refactor the JS↔Rust boundary, the async
> model, or the `RefCell<DarklyEngine>` borrow discipline: read this first.**
> Almost every "obvious simplification" in this area has already been tried and
> reverted, usually because it reintroduces a class of bug that is *invisible in
> headless tests and only reproduces in a real browser under GPU load.* This doc
> records what was tried, what replaced it, and what was deliberately given up to
> get here, so the next person doesn't pay the same tuition twice.

This is a living document. It explains the **why**, not the **what**; for the
current API surface see [getting-started-typescript.md](getting-started-typescript.md)
and [getting-started-rust.md](getting-started-rust.md); for the architecture in
the abstract see [`CONTRIBUTING.md`](../CONTRIBUTING.md).

---

## Part 0: Why Rust at all, and not pure TypeScript?

Darkly is a **GPU-native paint program**. The two hottest subsystems, the
node-graph brush engine (GPU compute pipelines, per-dab) and the layer compositor
(ping-pong blend, region tracking), are not "a bit of math in a hot loop"; they
are the product. Three forces pushed the core into Rust + WebAssembly rather than
TypeScript:

1. **One authoritative core, many frontends.** The document model (layer tree,
   modifiers, undo, serialization) must be reasoned about, tested, and evolved
   *without a GPU and without a browser*. That's the [Document Authority
   Principle](../CONTRIBUTING.md): the document is authoritative and serializable; the
   compositor is a derived realization. A Rust core compiles three ways: the
   WASM bridge for the browser, a future Tauri/native backend, and a **headless
   `cargo test`** harness that drives the real engine on Vulkan/Metal. A
   TypeScript core could not run the same logic in a native test or a native app;
   we'd be maintaining two engines and praying they agree.

2. **Correctness of a large, invariant-heavy model.** Undo/redo, the coordinate
   frames (see [coordinate-systems.md](coordinate-systems.md)), modifier stacks,
   and format I/O are exactly the kind of code where Rust's type system and borrow
   checker pay for themselves. The project's stated reason for existing is to
   resist architectural bloat (see the README's "Philosophy"); a strongly-typed
   core with enforced module boundaries is part of that bet.

3. **Performance with a portable GPU story.** WebGPU (via `wgpu`) gives one GPU
   abstraction that targets the browser *and* native from the same Rust. Graphite
   (credited in the README) proved out Rust + WASM + WebGPU for serious 2D
   tooling; Darkly follows that lineage.

**The cost we accepted:** everything that crosses the JS↔Rust boundary must be
serialized, and the boundary itself is the hardest part of the whole system,
which is the rest of this document. The frontend stays in Svelte/TypeScript
because UI is where TS is strongest; the dividing line is "model and pixels in
Rust, interface in TS."

---

## Part 1: The boundary problem, in one paragraph

`#[wasm_bindgen]` wraps every exported struct in an `Rc<RefCell<T>>`. A `&mut self`
method therefore takes an exclusive `borrow_mut()` for the call's duration. On a
single-threaded JS runtime that *looks* safe. It is not: **Chromium synchronously
pumps the browser event queue inside `queue.submit()` and
`surface.get_current_texture()`.** So while you are mid-call holding a borrow, a
pending `requestAnimationFrame` or pointer callback can fire *inside the GPU call*
and re-enter the engine. A second `borrow_mut()` then panics, and the panic
unwinds through wasm-bindgen **without dropping the guard**, permanently poisoning
the `RefCell`. Every later call fails for the rest of the session. This single
hazard (re-entrancy via the GPU event pump) has shaped every version of the
boundary since. It is **not reproducible headless** (native `wgpu` doesn't pump a
JS event loop), which is why it keeps almost-coming-back.

The rest of this doc is the four eras of fighting that hazard.

---

## Era 1: The naive bridge (Feb 2026, `b61cd87` → `219a879~1`)

```rust
#[wasm_bindgen]
pub struct DarklyHandle(DarklyEngine);   // every &mut self method = borrow_mut()
```

The first bridge was a thin one-to-one wrapper: one `#[wasm_bindgen]` method per
engine operation, each taking `&mut self`. Serialization conventions were already
thoughtful (JSON strings out, `serde_wasm_bindgen` in), but there was **no
re-entrancy protection at all**.

**Why it was scrapped:** the poison bug above. The concrete repro, preserved in
the commit that fixed it:

```text
stroke_to(&mut self)            ← wasm-bindgen borrow #1
  → brush engine → queue.submit()
    → Chromium pumps event queue
      → pending rAF fires
        → render(&mut self)     ← borrow #2: PANIC, guard leaked, handle dead
```

Painting while a frame was pending killed the editor permanently. This is the
canary the whole system is still measured against
([`tests/protocol_reentrancy.rs`](../crates/darkly/tests/protocol_reentrancy.rs)).

---

## Era 2: Interior mutability + command queue (Mar 2026, `219a879`)

The fix commit explicitly weighed three options (worth preserving because future
refactors tend to re-propose options 1 and 3):

1. **JS-side boolean "busy" guard.** Rejected: doesn't fix the root cause; every
   call site must remember to check, and one missed guard re-poisons the cell.
2. **Interior mutability + command queue (chosen).** Two layers:
   - *Interior mutability:* every `#[wasm_bindgen]` method takes `&self`.
     wasm-bindgen's outer `RefCell` then only ever does **shared** borrows (which
     can't conflict); the handle manages its own inner `RefCell`s with
     `try_borrow_mut()`, so a re-entrant call sees "busy" and skips instead of
     panicking. This kills the *permanent poison* failure mode outright.
   - *Command queue:* the hot conflict (`stroke_to` during painting vs `render`)
     is separated structurally: stroke ops push a `Command` onto a
     `RefCell<Vec<Command>>` **without touching the engine**; `render` drains the
     queue, then composites. Different RefCells ⇒ no borrow conflict, by
     construction rather than by luck.
3. **Split into `RenderHandle` + `PaintHandle`** with independent RefCells.
   Rejected as a much larger refactor (it means splitting `DarklyEngine` and
   sharing GPU context + document across handles).

Methods sorted into three buckets: ~40 *queued mutations* (fire-and-forget, zero
borrow), ~15 *direct mutations* (click-frequency, returned values, **panicked on
re-entrancy**, accepted as "a bug to fix structurally, not paper over"), and ~18
*queries*.

**Why it was scrapped:** it worked, but it was a **bespoke, hand-maintained
taxonomy**. Every new engine operation forced an author to pick a bucket, add a
`Command` variant, and wire a drain arm: a central `enum Command` and a central
`match` that grew without bound (the exact thing the [Modularity
Principle](../CONTRIBUTING.md) forbids). The ~15 "direct mutation" methods were a
standing latent panic. And there was still no uniform way to *return a value from
an operation that needs an async GPU readback* (copy, export, save); those were
special-cased. The command queue solved the stroke race; it did not give the
boundary a single, principled shape.

---

## Era 3: The unified request/response transport (Jun 2026, `78c229c`/`72f95f9`)

The command queue generalized into **one async message protocol**, the model
that's live today
([`engine/protocol/transport.rs`](../crates/darkly/src/engine/protocol/transport.rs),
[`frontend/src/engine/protocol.ts`](../frontend/src/engine/protocol.ts)). The
per-method wasm surface collapsed to three calls:

- **`enqueue(id, kind, payload, bytes)`**: appends to a FIFO. **Borrows
  nothing**, so a request fired re-entrantly inside `submit()`'s event pump just
  lands in the queue. *No borrow ⇒ no panic*: this is the load-bearing
  invariant.
- **`drain()`**: `try_borrow_mut`s, dispatches the FIFO, yields `Busy` if render
  holds the engine (caller reschedules). Driven by a `MessageChannel` macrotask.
- **`render(time)`**: drains under its borrow, then composites, same frame.

Each request carries an `id`; the JS side keeps an `id → { resolve, reject }`
table and turns the whole thing into ordinary `await engine.send('copy', {...})`.
Dispatch is **modular and auto-discovered**: a handler is
`fn(&mut DarklyEngine, Value, &[u8]) -> Result<Response, ProtocolError>`
registered by name; `build.rs` finds it. No central `match`, no bucket taxonomy.
The core `Transport` is **platform-agnostic** (no wasm types), so a future Worker
or Tauri wire reuses the same FIFO + registry: only the `id→promise` edge
changes.

This is the decisive architectural turn, and it answers **"why not just expose
ops as `async fn` via `wasm-bindgen-futures`?"**, the question every newcomer
asks. The answer is in [Part 2](#part-2--why-not-simple-async-wasm-bindgen).

**What it left unfinished:** a multi-step op that needs a GPU readback
(copy: warm the selection cache → kick a copy readback → build the clipboard)
still couldn't resolve *its own* promise. It had to stash a `pending_copy` field
and get hand-resumed by a central readback `match`. The transport made *requests*
async; it didn't make *deferred completions* first-class.

---

## Era 4: Deferred FFI resolution (Jun 2026, `2fda0d9`, "Phase A")

The current HEAD's groundwork. `Response::deferred()`, a `completed_requests`
buffer, and request-id threading (`set_current_request` / `resolve_request` /
`take_completed_requests`) let a handler say "I'll answer later" and have its GPU
readback resolve the original JS promise when the pixels land: no bespoke poll
path per op. `render` flushes `take_completed_requests()` each frame so deferred
results land the same frame the readback completes.

This is the last *shipped* era. It made deferred completion uniform but did **not**
yet remove the `pending_*` fields or the central resume `match`; that's the
in-progress work below.

---

## Era 5 (in progress): One engine-side, frame-driven scheduler

The active plan (`crates/darkly/src/engine/host/`, pilot on copy/cut) finishes the
arc. Today there are still **two unrelated async mechanisms**: the request
transport (Era 3) and the GPU readback scheduler. A multi-step op is a state
machine smeared across a `pending_copy` field, a `ReadbackContext` enum variant,
and a central resume `match`. The plan collapses both into **one current-thread,
frame-driven executor in the core**, so a deferred op becomes a *linear `async fn`*
that `.await`s readbacks and the `pending_*` smear becomes ordinary control flow.

The three constraints it must respect (and that any future refactor here inherits)
are worth stating plainly, because they are *why the design looks unusual*:

- **No borrow held across `.await`.** Engine access is only ever a *scoped
  synchronous closure* (`EngineCell::with(|e| …)`, returning `None` on
  contention). You cannot `.await` inside a sync closure, so "hold the engine
  across an await" is **structurally unrepresentable**, which matters because a
  borrow held across an await would block the very `device.poll` that completes
  the awaited readback (a deadlock).
- **No reactor; progress is frame-driven.** A GPU readback completes only because
  a frame called `device.poll`. So the executor uses a **hand-rolled no-op waker**
  and polls every task once per frame: no `tokio`, no `async-task`, no
  `wasm-bindgen-futures` in the core (that would also break platform-agnosticism).
- **Try-acquire-or-yield everywhere.** The Era-2 reentrancy lesson, promoted from
  "two sanctioned borrow sites" to a mechanical invariant: `EngineCell::with` is
  the *sole* sanctioned borrow, guarded by a CI grep gate.

See the plan for the full design, the considered-and-rejected alternatives, and
the abandon criteria.

---

## Part 2: Why not "simple async wasm-bindgen"?

This is the most-asked question, so it gets a direct answer. `wasm-bindgen-futures`
**works**: Darkly uses it (`session.createHandle` awaits a real Rust `async fn`).
So the boundary *can* do async. The question is why operations aren't exposed as
`async fn op() -> Data` bridged straight to JS promises, instead of going through
the id→promise transport.

First, dispel the premise it usually rides in on: **there is no synchronous,
thread-blocking op.** `copy()` returns immediately and resolves a promise frames
later; from TS you already `await` it. A *blocking* readback would deadlock on
WASM outright (you cannot block the event loop the GPU needs to make progress).
So the choice was never "async vs. a thread-blocking sync call"; both designs are
async-to-JS. The choice is **how the two sides are bridged.**

Two real designs exist:

| | **(A) Message-passing transport** (chosen) | **(B) Bridge Rust futures → JS promises** |
|---|---|---|
| Boundary | `enqueue` + `id→promise` table | `handle.copy()` returns the JS promise directly |
| Who drives the op | one engine-side, frame-driven executor | the JS microtask executor (`wasm-bindgen-futures`) |
| Waker | **no-op** (every task polled each frame) | **real wakers required**, wired from readback-completion into every parked future |
| Core deps | dep-free, platform-agnostic (Tauri/headless reuse it) | browser-only `wasm-bindgen-futures` in the core ⇒ fork the executor per platform |
| Schedulers | **one** (frame tick drives requests *and* readbacks) | **two** (rAF drives `device.poll`; JS loop drives tasks), bridged by wakers |
| Op signature | `await engine.send('copy', …)` (stringly; typed client is Phase C) | `await handle.copy(id)` (typed directly) |

Design (B) is coherent and not wrong, but it **re-creates the exact problem the
transport was built to remove**: two async mechanisms that don't know about each
other. A GPU readback only advances when a *frame* runs `device.poll`; if the op's
future is owned by the JS microtask loop, you must wire real wakers from readback
harvest back into each parked future, and you've split scheduling across the rAF
loop and the JS loop again. The frame-driven executor uses a no-op waker
*precisely because* it brute-force polls every task each frame: that simplification
is **incompatible** with handing the future to `wasm-bindgen-futures`, which never
re-polls without a real wake. So (B) is not *less* machinery; it is *go build the
waker plumbing (A) deliberately avoids*, plus a browser-only dependency in a core
that must also run on Tauri and in headless tests.

The honest one-line summary: **Darkly is two genuinely-async runtimes (the JS
event loop and a Rust frame-driven executor) joined by a message-passing boundary,
not by a shared future.** It is the actor model, the same shape as a Web Worker
behind `postMessage`. That is a real architectural choice with real costs, not a
sync core wearing an async hat.

---

## What was intentionally sacrificed

Future refactorers should know these are **deliberate**, not oversights:

- **Direct `async fn op() -> Data` signatures on the Rust surface.** Given up for
  the message-passing transport (Part 2). The op is still genuinely async
  internally; the boundary just isn't a future bridge.
- **Synchronous return values from GPU-reading ops.** Impossible by physics on
  WASM (no blocking readback); these are deferred and resolve a promise. See
  [No Blocking GPU Readbacks](../CONTRIBUTING.md).
- **Typed, per-method TS ergonomics, *for now*.** `await engine.send('copy', {…})`
  is stringly-typed. The typed client (`await engine.copy(id)`) is a planned thin
  wrapper *over* the transport (plan "Phase C"): it recovers the ergonomics
  without collapsing the two schedulers back into one.
- **A standard async runtime.** No `tokio`/`async-task`/`wasm-bindgen-futures` in
  the core: a hand-rolled, dep-free, `!Send`, frame-driven executor instead, to
  keep the core platform-agnostic and the waker model trivial.
- **A single `RefCell<DarklyEngine>` as one actor cell.** State is *not* scattered
  into per-subsystem cells to please the borrow checker; see the [Ownership
  Principle](../CONTRIBUTING.md). Splitting the engine into `RenderHandle`/`PaintHandle`
  (Era-2 option 3) was considered and rejected for the same reason.

---

## For the next person who wants to refactor this

1. **Reproduce the hazard in a browser before you touch anything.** The poison bug
   does not appear headless. `tests/protocol_reentrancy.rs` models the *borrow
   class*, but the real acceptance check is manual: paint while a rAF render is
   pending and confirm no poison. If your change can't survive that, it's wrong no
   matter how clean it reads.
2. **The invariants are load-bearing, not stylistic:** `enqueue` borrows nothing;
   every other engine access is try-acquire-or-yield; no borrow crosses an
   `.await`. Each one maps to a specific past outage.
3. **"Just use `wasm-bindgen-futures`" has an answer: Part 2.** If you still think
   it's right, the burden is to show how one frame-driven `device.poll` wakes
   JS-loop-owned futures *without* re-forking the scheduler or dragging a
   browser-only dep into the platform-agnostic core.
4. **The direction of travel is fewer mechanisms, more modularity**: one
   transport, one scheduler, auto-discovered handlers, no central `match`. A
   refactor that adds a second dispatch path or a hand-maintained taxonomy is
   moving against the grain; the last three eras were all about deleting exactly
   that.

---

## Commit map

| Era | Commit(s) | Date | What |
|---|---|---|---|
| 1 | `b61cd87` "let there be darkly" | Feb 21 2026 | Naive `DarklyHandle(DarklyEngine)`, no reentrancy protection |
| 2 | `219a879` "re-entrancy protection and misc bugfixes" | Mar 18 2026 | Interior mutability + command queue; 3 options weighed |
| 3 | `78c229c` / `72f95f9` "wasm async request/response transport rework" | Jun 14 2026 | Unified id→promise transport; modular auto-discovered handlers |
| 4 | `2fda0d9` "clean up rust async architecture" | Jun 22 2026 | Deferred FFI resolution (`Response::deferred`, `completed_requests`), plan Phase A |
| 5 | *in progress* | N/A | Engine-side frame-driven scheduler; ops become linear `async fn` |
