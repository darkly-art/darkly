/**
 * Tool-session-scoped engine access: the primitive that makes "an async tool
 * op resumes into a changed world" unrepresentable.
 *
 * The problem it solves: a tool hook parks on an `await` (an engine round-trip),
 * and while it's suspended the world changes underneath it: the tool is
 * switched or the active layer changes. The same synchronous moment that
 * invalidates the session also nulls the tool's state (its gizmo, its
 * placement). On resume the hook dereferences state that died with the session,
 * or drives an engine op against the wrong layer.
 *
 * The fix is a capability, not a discipline: a tool reaches the engine only
 * through the *current* {@link SessionEngine}. A dead session's `send` rejects
 * with {@link ToolSessionCancelled} the instant its response lands, unwinding
 * the caller before it can touch state that no longer belongs to it. So:
 *
 *   Reaching any line after an `await` proves the session is still alive, which
 *   proves the tool state is still valid. No per-call-site guard is needed;
 *   the await *is* the check.
 *
 * **Ownership is per-instance.** Tool state and the session both live on the
 * `DarklyInstance` (`state/app.svelte.ts`): each editor tab owns its own tool
 * objects and its own session, so a background tab finishing async init can
 * never steal the focused tab's session, and two tabs never alias one gizmo.
 * The session's lifecycle is owned by the single tool-transition effect in
 * `CanvasView.svelte` (which applies a {@link planToolTransition}) plus the
 * tab-close teardown; it is never touched per-tool. Tools hold their instance
 * and reach the live session through it: {@link SessionEngine} is the same
 * cancellation primitive it always was; only its owner moved.
 *
 * Residual: cancellation is automatic only for awaits routed through the session
 * engine. In today's tooling every await crosses the engine (directly or via a
 * gizmo/binding), so it's fully covered. A future hook that awaits a *non*-engine
 * promise (a timer, a `fetch`) would not auto-cancel; it must re-check its
 * session on resume, or route its wait through the engine. A finer-grained
 * sibling of this idiom is the brush tool's `hoverGen`, which invalidates on
 * stroke start too (a tighter boundary than a whole session).
 *
 * Accepted residual (unchanged from prior behaviour): a session killed
 * mid-stroke drops `strokeTo`/`endStroke`; the engine tolerates this:
 * `begin_stroke` overwrites `active_stroke_layer` unconditionally
 * (crates/darkly/src/engine/painting.rs).
 *
 * **The rejection guard.** A bare `void tool.someAsyncHook()` that isn't
 * dispatched through {@link runHook} surfaces its `ToolSessionCancelled` as an
 * unhandled rejection. {@link setupToolSessionRejectionGuard} installs a
 * window-level backstop that swallows exactly that rejection (and logs it in
 * dev, so unwrapped spawns stay discoverable). `runHook` remains the local
 * convention at the dispatcher's own call sites, and keeps Vitest's node env
 * deterministic, where there is no `window`.
 */
import type { Engine, EngineRequests } from '../engine/protocol';
import { makeApi, type EngineApi } from '../engine/protocol_gen';

/** Thrown by a dead session's request when its response resolves. Swallowed by
 *  {@link runHook} at the dispatcher's hook call sites: a cancelled op is a
 *  no-op, not an error. */
export class ToolSessionCancelled extends Error {
    constructor() {
        super('tool session cancelled');
        this.name = 'ToolSessionCancelled';
    }
}

/** A thin, cancellation-aware wrapper over the real {@link Engine}, bound to one
 *  tool session. Tool code holds no direct `Engine` reference: it reaches the
 *  engine only through this session's typed {@link api}, so the resource itself
 *  enforces safety. The `api` is a second {@link makeApi} client over the inner
 *  engine's transport, wrapped so a dead session rejects awaited requests and
 *  drops fire-and-forget ones. */
export class SessionEngine implements EngineRequests {
    #alive = true;
    readonly api: EngineApi;

    constructor(inner: Engine) {
        const t = inner.transport;
        this.api = makeApi({
            request: (kind, payload, bytes) =>
                t.request(kind, payload, bytes).then((v) => {
                    // The one cancellation point: reaching any line past an
                    // await routed through the session proves it's still alive.
                    if (!this.#alive) throw new ToolSessionCancelled();
                    return v;
                }),
            postFF: (kind, payload, bytes) => {
                // A dead session drops it; its effect is moot.
                if (this.#alive) t.postFF(kind, payload, bytes);
            },
        });
    }

    /** Sever this session. In-flight requests reject on resume; new fire-and-forget
     *  posts drop. */
    kill(): void {
        this.#alive = false;
    }
}

/** Run an async hook, swallowing {@link ToolSessionCancelled} (and only that,
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

let guardWired = false;

/** Install a window-level backstop that swallows an unhandled
 *  {@link ToolSessionCancelled}: the safety net for any bare `void
 *  tool.asyncHook()` spawn that skipped {@link runHook}. Idempotent; mirrors
 *  `setupModifierCursorTracking`. In dev it logs the swallowed rejection so an
 *  unwrapped spawn stays discoverable. No-op where there's no `window` (Vitest's
 *  node env), so `runHook` stays the deterministic path there. */
export function setupToolSessionRejectionGuard(): void {
    if (guardWired) return;
    guardWired = true;
    if (typeof window === 'undefined') return;
    window.addEventListener('unhandledrejection', (e: PromiseRejectionEvent) => {
        if (e.reason instanceof ToolSessionCancelled) {
            e.preventDefault();
            if (import.meta.env?.DEV) {
                console.debug('swallowed unhandled ToolSessionCancelled', e.reason);
            }
        }
    });
}

/** A snapshot of the transition inputs the tool-session lifecycle keys on. Pure
 *  data so {@link planToolTransition} is unit-testable without the editor. */
export interface ToolTransitionState {
    hasEngine: boolean;
    hasCanvas: boolean;
    toolId: string;
    layerId: number | null;
    reactivations: number;
}

/** The actions a single tool transition resolves to. Applied in field order by
 *  the `CanvasView` transition effect: deactivate the outgoing tool through its
 *  still-alive session, `rebind` (begin a fresh session), then `activate` /
 *  `dismiss` the incoming tool through it. `kill` short-circuits everything:
 *  there's no engine/canvas to bind yet. */
export interface ToolTransitionPlan {
    /** Sever the session and do nothing else (no engine or canvas yet). The
     *  effect re-fires when they appear, so this never wedges. */
    kill: boolean;
    /** Deactivate the outgoing tool through its still-alive session. */
    deactivate: boolean;
    /** Begin a fresh session (kills the prior one). */
    rebind: boolean;
    /** Activate the incoming tool through the fresh session. */
    activate: boolean;
    /** Dismiss the active tool's overlay through the fresh session. */
    dismiss: boolean;
}

const NO_OP: ToolTransitionPlan = {
    kill: false,
    deactivate: false,
    rebind: false,
    activate: false,
    dismiss: false,
};

/**
 * The per-instance tool transition table (no focus dimension; the session is
 * per-`DarklyInstance`, so focus falls out entirely). Conditions are checked in
 * order; the first match wins.
 *
 * | Condition | Plan |
 * |---|---|
 * | No engine or no canvas | `kill` (effect re-fires when they appear) |
 * | Tool changed | `deactivate` old, `rebind`, `activate` (subsumes a simultaneous layer change) |
 * | Reactivation requested (same tool) | `rebind` + `activate`, no `deactivate` (would commit the floating just pasted) |
 * | Layer changed | `rebind` + `dismiss` |
 * | Nothing changed | no-op |
 */
export function planToolTransition(
    prev: ToolTransitionState,
    next: ToolTransitionState,
): ToolTransitionPlan {
    if (!next.hasEngine || !next.hasCanvas) return { ...NO_OP, kill: true };
    if (prev.toolId !== next.toolId) {
        return { ...NO_OP, deactivate: true, rebind: true, activate: true };
    }
    if (prev.reactivations !== next.reactivations) {
        return { ...NO_OP, rebind: true, activate: true };
    }
    if (prev.layerId !== next.layerId) {
        return { ...NO_OP, rebind: true, dismiss: true };
    }
    return { ...NO_OP };
}
