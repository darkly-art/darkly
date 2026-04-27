import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';
import { toolRegistry } from '../tools/registry';

export interface Color {
    r: number; g: number; b: number; a: number;
}

class AppState {
    handle = $state<DarklyHandle | null>(null);

    // Colors
    foreground = $state<Color>({ r: 0, g: 0, b: 0, a: 255 });
    background = $state<Color>({ r: 255, g: 255, b: 255, a: 255 });

    // Active tool
    activeToolId = $state<string>('brush');

    // Active layer
    activeLayerId = $state<number | null>(null);

    // Mask editing — which layer's mask is the current paint target (null = editing layer content)
    editingMaskLayerId = $state<number | null>(null);

    // Tool runtime state -- working values adjusted while painting.
    fillTolerance = $state(32);     // 0-255
    fillAll = $state(false);
    gradientType = $state<'linear' | 'radial'>('linear');

    // Layer tree (read from WASM, refreshed after mutations/undo/redo).
    layerTree = $state<any[]>([]);

    // Veil list (read from WASM, refreshed after mutations).
    veilList = $state<any[]>([]);

    // View transform (controlled by canvas navigation)
    panX = $state(0);
    panY = $state(0);
    zoom = $state(1.0);
    rotation = $state(0);   // radians

    // Tool cursor — when non-null, overrides nav cursor on the canvas element.
    toolCursor = $state<string | null>(null);

    swapColors() {
        const tmp = { ...this.foreground };
        this.foreground = { ...this.background };
        this.background = tmp;
    }

    resetColors() {
        this.foreground = { r: 0, g: 0, b: 0, a: 255 };
        this.background = { r: 255, g: 255, b: 255, a: 255 };
    }

    refreshLayerTree() {
        if (this.handle) {
            try {
                const tree = JSON.parse(this.handle.layer_tree());
                this.layerTree = Array.isArray(tree) ? tree : [];
            } catch { this.layerTree = []; }
        }
    }

    refreshVeilList() {
        if (this.handle) {
            try {
                const list = JSON.parse(this.handle.veil_list());
                this.veilList = Array.isArray(list) ? list : [];
            } catch { this.veilList = []; }
        }
    }

    // --- Async copy result callback ---

    private _copyCallback: ((result: any) => void) | null = null;

    /** Register a one-shot callback for when the async copy readback completes. */
    onCopyResult(cb: (result: any) => void) {
        this._copyCallback = cb;
        this.requestFrame();
    }

    // --- Demand-driven rendering ---

    private _framePending = false;

    /**
     * Number of active UI interactions (panel drags, slider adjustments,
     * etc.) that should suppress continuous animation rendering.  While
     * non-zero, `requestFrame()` still runs one-shot requests (e.g. from
     * tool actions) but will NOT self-schedule the next animation frame.
     * This keeps the main thread free for pointer events so that panels
     * like the brush builder remain responsive during animated veils.
     */
    private _interactionCount = 0;

    /** Call when a sustained UI interaction starts (e.g. node drag). */
    beginInteraction() { this._interactionCount++; }

    /** Call when it ends.  Resumes animation rendering if needed. */
    endInteraction() {
        this._interactionCount = Math.max(0, this._interactionCount - 1);
        if (this._interactionCount === 0) this.requestFrame();
    }

    /** Schedule a render frame if one isn't already pending. */
    requestFrame() {
        if (this._framePending) return;
        this._framePending = true;
        requestAnimationFrame((ts) => {
            this._framePending = false;
            if (!this.handle) return;
            const needsMore = this.handle.render(ts / 1000.0);

            // Per-frame tool hook — async state sync (e.g. GPU readback completion).
            toolRegistry.get(this.activeToolId)?.onFrame?.();

            // Check for completed async copy/cut readback.
            if (this._copyCallback) {
                const result = this.handle.poll_copy_result();
                if (result) {
                    const cb = this._copyCallback;
                    this._copyCallback = null;
                    cb(result);
                }
            }

            // Continue animation loop only when no UI interaction is
            // monopolizing the main thread.  One-shot renders (tool
            // actions, resize, etc.) always go through — only the
            // self-scheduling continuous loop is suppressed.
            const shouldContinue = needsMore || this._copyCallback;
            if (shouldContinue && this._interactionCount === 0) {
                this.requestFrame();
            }
        });
    }
}

export const app = new AppState();

// `app` owns the live DarklyHandle. HMR'ing this module resets `handle` to
// null, orphaning the running engine. Force a full reload instead.
if (import.meta.hot) {
    import.meta.hot.accept(() => import.meta.hot!.invalidate());
}
