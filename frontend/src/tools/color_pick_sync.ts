import type { Engine } from '../engine/protocol';
import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';

// pick_color queues an async GPU readback in the engine; the picked color lands
// in the engine's cache a frame or two later. Under the async transport we poll
// `has_pending_color_pick` and, once it clears, read `last_picked_color` and
// commit it. `pollInFlight` guards against overlapping per-frame poll chains.
//
// Module-local — only one pick can be in flight globally at a time. Both the
// colorpicker tool and the modifier-held `sampleColor` action share this state
// (they never coexist mid-pick: only one pointer down at a time).
let waitingForPick = false;
let pollInFlight = false;

/** Queue an async color pick at canvas-space (cx, cy).
 *
 *  Reads `tools.colorPickerSampleSource` to decide between merged-composite
 *  and current-layer sampling. The Rust side falls back to the merged
 *  composite when the current-layer source can't resolve (group, mask, point
 *  outside layer extent), so this never silently no-ops. */
export function startPick(engine: Engine, cx: number, cy: number): void {
    const mode = config.get('tools.colorPickerSampleSource');
    const layerId =
        mode === 'currentLayer' && app.activeLayerId != null ? app.activeLayerId : -1;
    engine.post('pick_color', { x: cx, y: cy, id: layerId });
    waitingForPick = true;
}

/** Per-frame poll. Commits the picked color to `app.foreground` once the GPU
 *  readback lands. Called unconditionally from the app's frame loop so the
 *  modifier-held pick works regardless of which tool is active. Promise-driven:
 *  fires at most one query chain at a time. */
export function pollPick(): void {
    if (!waitingForPick || pollInFlight) return;
    const engine = app.engine;
    if (!engine) return;
    pollInFlight = true;
    engine
        .send<boolean>('has_pending_color_pick')
        .then((pending) => {
            if (pending) {
                pollInFlight = false;
                return; // still in flight; retry next frame
            }
            return engine
                .send<{ bytes: Uint8Array }>('last_picked_color')
                .then(({ bytes }) => {
                    waitingForPick = false;
                    pollInFlight = false;
                    if (!bytes || bytes.length < 4) return;
                    // Alpha-zero guard: sampling outside a layer's extent, on a
                    // fully transparent pixel, or from an unsupported texture
                    // format yields [0,0,0,0]. Writing that would silently set
                    // the foreground to opaque black (the UI discards alpha).
                    // Krita and Photoshop both ignore transparent picks.
                    if (bytes[3] === 0) return;
                    app.foreground = { r: bytes[0], g: bytes[1], b: bytes[2], a: bytes[3] };
                });
        })
        .catch(() => {
            pollInFlight = false;
        });
}
