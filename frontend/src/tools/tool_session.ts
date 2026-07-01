/**
 * Tool-session-scoped engine access — the primitive that makes "an async tool
 * op resumes into a changed world" unrepresentable.
 *
 * The problem it solves: a tool hook parks on an `await` (an engine round-trip),
 * and while it's suspended the world changes underneath it — the tool is
 * switched, the active layer changes, or (multi-tab) the focused instance swaps.
 * The same synchronous moment that invalidates the session also nulls the
 * tool's module-level state (its gizmo, its placement). On resume the hook
 * dereferences state that died with the session, or drives an engine op against
 * the wrong layer/tab.
 *
 * The fix is a capability, not a discipline: a tool reaches the engine only
 * through the *current* {@link SessionEngine}. A dead session's `send` rejects
 * with {@link ToolSessionCancelled} the instant its response lands, unwinding
 * the caller before it can touch state that no longer belongs to it. So:
 *
 *   Reaching any line after an `await` proves the session is still alive — which
 *   proves the module state is still valid. No per-call-site guard is needed;
 *   the await *is* the check.
 *
 * The events that invalidate a session are owned by the dispatcher in ~3 places
 * (tool change / active-layer change in `CanvasView.svelte`, instance swap in
 * `setActiveInstance`), never per-tool.
 *
 * Residual: cancellation is automatic only for awaits routed through the session
 * engine. In today's transform tooling *every* await crosses the engine
 * (directly or via a gizmo/binding), so it's fully covered. A future hook that
 * awaits a *non*-engine promise (a timer, a `fetch`) would not auto-cancel — it
 * must re-check `toolEngine()` on resume, or route its wait through the engine.
 * A finer-grained sibling of this idiom is the brush tool's `hoverGen`, which
 * invalidates on stroke start too — a tighter boundary than a whole session.
 */
import type { Engine, EngineRequests, RequestKind } from '../engine/protocol';

/** Thrown by a dead session's `send` when its response resolves. Swallowed by
 *  {@link runHook} at the dispatcher's hook call sites — a cancelled op is a
 *  no-op, not an error. */
export class ToolSessionCancelled extends Error {
    constructor() {
        super('tool session cancelled');
        this.name = 'ToolSessionCancelled';
    }
}

/** A thin, cancellation-aware wrapper over the real {@link Engine}, bound to one
 *  tool session. Tool code holds no direct `Engine` reference — it goes through
 *  the live session, so the resource itself enforces safety. */
export class SessionEngine implements EngineRequests {
    #inner: Engine;
    #alive = true;

    constructor(inner: Engine) {
        this.#inner = inner;
    }

    /** Sever this session. In-flight `send`s reject on resume; new `post`s drop. */
    kill(): void {
        this.#alive = false;
    }

    /** Awaited request. Resolves normally only if the session is still alive
     *  when the response lands; otherwise rejects with {@link ToolSessionCancelled},
     *  unwinding the caller before it can touch state that died with the session.
     *  This is the one cancellation point — there is no per-call-site guard
     *  anywhere else. */
    send<T = any>(kind: RequestKind, payload?: object, bytes?: Uint8Array): Promise<T> {
        return this.#inner.send<T>(kind, payload, bytes).then((v) => {
            if (!this.#alive) throw new ToolSessionCancelled();
            return v;
        });
    }

    /** Fire-and-forget request. A dead session drops it — its effect is moot. */
    post(kind: RequestKind, payload?: object, bytes?: Uint8Array): void {
        if (this.#alive) this.#inner.post(kind, payload, bytes);
    }
}

/** The single live tool session — global, matching today's module-level tool
 *  state (one gizmo shared across tabs). */
let current: SessionEngine | null = null;

/** Start a fresh session over `engine`, killing any prior one. Returns the new
 *  session so a caller can address it directly (e.g. an `onActivate` binding). */
export function beginToolSession(engine: Engine): SessionEngine {
    current?.kill();
    return (current = new SessionEngine(engine));
}

/** Kill the live session, if any, and leave none. */
export function killToolSession(): void {
    current?.kill();
    current = null;
}

/** The live session's engine, or `null` when no tool session is active. */
export function toolEngine(): SessionEngine | null {
    return current;
}

/** Run an async hook, swallowing {@link ToolSessionCancelled} (and only that —
 *  real errors still propagate). Applied by the dispatcher at each hook call
 *  site: a hook whose op was cancelled mid-flight settles cleanly instead of
 *  surfacing a rejection. Accepts sync hooks too (they wrap trivially). */
export function runHook(result: unknown): Promise<void> {
    return Promise.resolve(result).then(
        () => {},
        (e) => {
            if (!(e instanceof ToolSessionCancelled)) throw e;
        },
    );
}
