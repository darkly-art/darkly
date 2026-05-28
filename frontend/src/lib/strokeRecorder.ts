/**
 * Pen-input recorder. Gated on the `?_RECORD_STROKES=1` URL parameter.
 *
 * When enabled, captures every brush_stroke event of a single stroke
 * (begin -> N moves -> end) into an in-memory buffer and auto-downloads
 * the buffer as a JSON file when the stroke ends. One stroke per file.
 *
 * Output is consumed by `cargo run --bin stroke_replay_bench` for
 * deterministic perf-bench replay of real tablet input.
 *
 * **Perf cost when disabled:** `init()` reads the URL search once at boot
 * and stores `enabled = false`. Every recorder method early-returns on
 * that flag before doing any work — one boolean check per pointer event,
 * no allocations, no buffer growth, no event projection. The brush tool
 * calls `beginStroke / addEvent / endStroke` unconditionally; the cost
 * when disabled is below the measurement floor.
 */

import { app } from '../state/app.svelte';
import { downloadFile } from './downloadFile';

export interface RecordedEventParams {
    x: number;
    y: number;
    pressure: number;
    x_tilt: number;
    y_tilt: number;
    rotation: number;
    tangential_pressure: number;
    time_ms: number;
    cr: number;
    cg: number;
    cb: number;
    ca: number;
}

interface RecordingFile {
    version: 1;
    recorded_at: string;
    canvas_width: number;
    canvas_height: number;
    events: RecordedEventParams[];
}

class StrokeRecorder {
    private enabled = false;
    private events: RecordedEventParams[] = [];
    private canvasWidth = 0;
    private canvasHeight = 0;

    init(): void {
        const search =
            typeof globalThis !== 'undefined' && (globalThis as { location?: Location }).location
                ? (globalThis as { location: Location }).location.search
                : '';
        this.enabled = new URLSearchParams(search).get('_RECORD_STROKES') === '1';
    }

    /** True iff `?_RECORD_STROKES=1`. Exposed for tests / UI affordances. */
    get isEnabled(): boolean {
        return this.enabled;
    }

    beginStroke(canvasWidth: number, canvasHeight: number, firstEvent: RecordedEventParams): void {
        if (!this.enabled) return;
        this.canvasWidth = canvasWidth;
        this.canvasHeight = canvasHeight;
        this.events = [pickEventFields(firstEvent)];
    }

    addEvent(event: RecordedEventParams): void {
        if (!this.enabled) return;
        if (this.events.length === 0) return;
        this.events.push(pickEventFields(event));
    }

    endStroke(): void {
        if (!this.enabled) return;
        if (this.events.length === 0) return;
        const data: RecordingFile = {
            version: 1,
            recorded_at: new Date().toISOString(),
            canvas_width: this.canvasWidth,
            canvas_height: this.canvasHeight,
            events: this.events,
        };
        this.events = [];
        downloadFile(
            JSON.stringify(data),
            `darkly-stroke-${data.recorded_at.replace(/[:.]/g, '-')}.json`,
            'application/json',
        );
    }
}

/** Read the current document's canvas dimensions from the JS-side mirror
 *  that `actions/index.ts` keeps in sync with the engine. Avoids re-entering
 *  the WASM RefCell mid-stroke. */
export function currentCanvasDimensions(): [number, number] | null {
    if (!app.handle) return null;
    return [app.docW, app.docH];
}

/** Snapshot exactly the fields we want in the recording. The brush tool's
 *  `brushStrokeParams` object is shared by reference with the WASM bridge,
 *  which mutates it to inject a serde `op` tag for deserialization. Without
 *  this projection that mutation leaks back into the recorded JSON. */
function pickEventFields(e: RecordedEventParams): RecordedEventParams {
    return {
        x: e.x,
        y: e.y,
        pressure: e.pressure,
        x_tilt: e.x_tilt,
        y_tilt: e.y_tilt,
        rotation: e.rotation,
        tangential_pressure: e.tangential_pressure,
        time_ms: e.time_ms,
        cr: e.cr,
        cg: e.cg,
        cb: e.cb,
        ca: e.ca,
    };
}

export const strokeRecorder = new StrokeRecorder();
