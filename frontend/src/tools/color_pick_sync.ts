import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';
import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';

// pick_color queues an async GPU readback and returns the *previous* cached
// result synchronously. Consuming the sync return would apply the prior pick's
// color, making every click feel one step behind. Instead, flag that a pick is
// in flight and commit the real color in pollPick once the readback lands.
//
// Module-local — only one pick can be in flight globally at a time. Both the
// colorpicker tool and the modifier-held `sampleColor` action share this state
// (they never coexist mid-pick: only one pointer down at a time).
let waitingForPick = false;

/** Queue an async color pick at canvas-space (cx, cy).
 *
 *  Reads `tools.colorPickerSampleSource` to decide between merged-composite
 *  and current-layer sampling. The Rust side falls back to the merged
 *  composite when the current-layer source can't resolve (group, mask, point
 *  outside layer extent), so this never silently no-ops. */
export function startPick(handle: DarklyHandle, cx: number, cy: number): void {
    const mode = config.get('tools.colorPickerSampleSource');
    const layerId =
        mode === 'currentLayer' && app.activeLayerId != null
            ? app.activeLayerId
            : -1;
    handle.pick_color(cx, cy, layerId);
    waitingForPick = true;
}

/** Per-frame poll. Commits the picked color to `app.foreground` once the
 *  GPU readback lands. Called unconditionally from the app's frame loop so
 *  the modifier-held pick works regardless of which tool is active. */
export function pollPick(): void {
    if (!waitingForPick || !app.handle) return;
    if (app.handle.has_pending_color_pick()) return;
    const rgba = app.handle.last_picked_color();
    waitingForPick = false;
    if (rgba.length < 4) return;
    // Alpha-zero guard: sampling outside a layer's extent, on a fully
    // transparent pixel, or from an unsupported texture format yields
    // [0,0,0,0]. Writing that would silently set the foreground to opaque
    // black (foreground discards alpha in the UI). Krita and Photoshop both
    // ignore transparent picks; match that behavior.
    if (rgba[3] === 0) return;
    app.foreground = { r: rgba[0], g: rgba[1], b: rgba[2], a: rgba[3] };
}
