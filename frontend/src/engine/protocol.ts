import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';
import { makeApi, type EngineApi, type RequestKind, type Transport } from './protocol_gen';

export type { RequestKind, EngineApi };

/** A protocol-level rejection envelope (`{ kind, message }`) thrown when a
 *  request can't be routed or decoded, or when a handler surfaces a domain
 *  error (the old `Result<_, JsError>` throw paths). */
export interface EngineError {
    kind: 'unknown_request' | 'bad_payload' | 'engine_error';
    message: string;
}

/** Default sink for `post()` rejections — a fire-and-forget request that fails
 *  surfaces here instead of being swallowed by a bare `void`. */
export function reportEngineError(e: unknown): void {
    const err = e as Partial<EngineError> | undefined;
    console.error('[engine] request failed:', err?.kind ?? 'error', err?.message ?? e);
}

interface Pending {
    resolve: (value: any) => void;
    reject: (reason: EngineError) => void;
}

/** One dispatched request's result, marshalled from the wasm `drain`/`render`. */
interface RawResult {
    id: number;
    value?: unknown;
    bytes?: Uint8Array;
    error?: EngineError;
}

interface DrainResult {
    busy: boolean;
    results?: RawResult[];
}

/** Cached, synchronously-readable snapshot of engine state the frontend mirrors,
 *  returned by {@link Engine.render} each frame. One struct for every value the
 *  UI caches (frame/thumbnail counters + document bools) — they ride together
 *  because they all exist for the same reason (mirroring), not as a handful of
 *  loose return scalars. The UI reads this (sync) instead of awaiting per-value
 *  engine queries; grow it as the UI needs more. Mirrors the Rust `EngineState`. */
export interface EngineState {
    frameCount: number;
    thumbnailVersion: number;
    dirty: boolean;
    hasSelection: boolean;
}

interface FrameStatus {
    busy: boolean;
    needsMore: boolean;
    /** Engine-state mirror (absent on a `busy` re-entrant render). */
    state?: EngineState;
    results?: RawResult[];
}

/** The async request/response boundary to the Darkly engine — the in-process
 *  transport. Wraps a wasm {@link DarklyHandle}: callers `send`/`post` requests
 *  by kind and the engine resolves them on a scheduled drain or at frame time.
 *
 *  Two drain trigger points share one FIFO: a `MessageChannel` macrotask
 *  (armed on first enqueue, drains promptly between frames) and {@link render}
 *  (drains under its borrow before compositing, for frame coherence). The
 *  scheduler yields to render — a busy drain reschedules instead of blocking.
 *
 *  Worker/Tauri backends (P2/P3) reuse this same id→promise table over a
 *  different wire; only the `enqueue`/`drain` hop changes. */
export class Engine {
    private readonly handle: DarklyHandle;
    private readonly pending = new Map<number, Pending>();
    private nextId = 1;
    private drainScheduled = false;
    private readonly channel: MessageChannel;

    /** The typed, per-kind request surface — the only public request API.
     *  Generated from the engine's method signatures (`protocol_gen.ts`);
     *  closes over this transport's private request/postFF hop. */
    readonly api: EngineApi;

    constructor(handle: DarklyHandle) {
        this.handle = handle;
        this.channel = new MessageChannel();
        this.channel.port1.onmessage = () => this.runScheduledDrain();
        const transport: Transport = {
            request: (kind, payload, bytes) => this.#request(kind, payload ?? {}, bytes),
            postFF: (kind, payload, bytes) => this.#postFF(kind, payload ?? {}, bytes),
        };
        this.api = makeApi(transport);
    }

    /** Awaited path — resolves with the response value (a binary response
     *  resolves with the JSON value plus a `bytes: Uint8Array` field). Rejects
     *  with an {@link EngineError} on protocol/handler failure. Private: the
     *  only public request surface is the typed {@link api}. */
    #request<T = any>(kind: RequestKind, payload: object = {}, bytes?: Uint8Array): Promise<T> {
        const id = this.nextId++;
        const promise = new Promise<T>((resolve, reject) => {
            this.pending.set(id, { resolve, reject });
        });
        this.handle.enqueue(id, kind, payload, bytes);
        this.armDrain();
        return promise;
    }

    /** Fire-and-forget path — for pointer-frequency mutations. Routes rejections
     *  to {@link reportEngineError} instead of a bare `void`, so a failed
     *  enqueue is logged rather than silently dropped. Submission order is
     *  preserved regardless of `postFF`/`request` interleaving (single FIFO). */
    #postFF(kind: RequestKind, payload: object = {}, bytes?: Uint8Array): void {
        this.#request(kind, payload, bytes).catch(reportEngineError);
    }

    /** Render a frame. Drains the FIFO under render's borrow, resolves those
     *  results, and returns the frame status (`needsMore` + the frame/thumbnail
     *  counters that used to be separate borrowing reads). `busy` is true when a
     *  re-entrant render couldn't get the borrow — caller must not reschedule. */
    render(timeSecs: number): FrameStatus {
        const status = this.handle.render(timeSecs) as FrameStatus;
        if (!status.busy && status.results) this.resolveResults(status.results);
        return status;
    }

    /** Queue an `ImageBitmap` frame upload for a camera-style void. The one
     *  engine op that can't cross the serialized protocol (an `ImageBitmap`
     *  isn't JSON) — still deferred, applied at the next drain/render. The
     *  bridge closes the bitmap once the copy is recorded. */
    uploadVoidExternalImage(layerId: number, bitmap: ImageBitmap): void {
        this.handle.upload_void_external_image(layerId, bitmap);
    }

    /** Engine-side default thumbnail dimension (compile-time constant; no
     *  engine borrow). The frontend's `THUMB_SIZE` must match. */
    engineDefaultThumbSize(): number {
        return this.handle.engine_default_thumb_size();
    }

    /** Release the underlying wasm handle (wasm-bindgen destructor). */
    free(): void {
        this.handle.free();
    }

    private armDrain(): void {
        if (this.drainScheduled) return;
        this.drainScheduled = true;
        this.channel.port2.postMessage(null);
    }

    private runScheduledDrain(): void {
        this.drainScheduled = false;
        const out = this.handle.drain() as DrainResult;
        if (out.busy) {
            // Render holds the borrow; try again on the next macrotask.
            this.armDrain();
            return;
        }
        if (out.results) this.resolveResults(out.results);
    }

    private resolveResults(results: RawResult[]): void {
        for (const r of results) {
            const p = this.pending.get(r.id);
            if (!p) continue;
            this.pending.delete(r.id);
            if (r.error) {
                p.reject(r.error);
                continue;
            }
            let value = r.value;
            if (r.bytes !== undefined) {
                // Binary response: attach bytes onto the JSON value (or expose
                // a bare `{ bytes }` when the value is null, e.g. color pick).
                if (value === null || value === undefined) value = { bytes: r.bytes };
                else (value as Record<string, unknown>).bytes = r.bytes;
            }
            p.resolve(value);
        }
    }
}
