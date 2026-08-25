/**
 * Opening a paint stroke, with the engine's refusal surfaced to the user.
 *
 * `begin_stroke` is the one gate every paint op passes through — brush, fill,
 * gradient, and anything that reaches the canvas through a `StrokeOp`. It
 * refuses a target whose pixels are generated (a smart object, a camera void, a
 * text layer) or one that is locked, and it answers with the reason. Without
 * this helper that reason lands in a `console.error` and the user sees a brush
 * that silently does nothing.
 *
 * Fire-and-forget by design: the request is queued ahead of the `stroke_to`
 * that follows it (one FIFO, order preserved), so the caller stays synchronous
 * and the toast arrives whenever the drain resolves.
 */
import type { EngineRequests } from '../engine/protocol';
import { toast } from '../state/toast.svelte';
import { ToolSessionCancelled } from './tool_session';

export function beginPaintStroke(engine: EngineRequests, layerId: number): void {
    void engine.api.beginStroke({ id: layerId }).catch((e: unknown) => {
        // A stroke opened through a session that died before the response
        // landed is a no-op, not a refusal — nothing to tell the user.
        if (e instanceof ToolSessionCancelled) return;
        toast.show('warning', (e as { message?: string })?.message ?? String(e));
    });
}
