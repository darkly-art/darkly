/**
 * Process-recording service — the per-tab lifecycle around the engine's
 * passive capture and the encoder worker. For each open tab (when
 * `recording.enabled`): negotiate an encoder config against the document,
 * spawn the worker on the tab's OPFS scratch, arm the engine's capture via
 * `set_recording_params`, and drain captured frames to the worker from the
 * tab's render loop (`pollFrame`).
 *
 * The scratch (`recording-scratch/<sessionId>~<recoveryId>/`) is the
 * working recording between saves; `.darkly` saves embed it and opens
 * re-seed it (see `storage/saveDocument.ts` / `actions/index.ts`). Its
 * lifecycle mirrors the recovery snapshot store's exactly: cleared on
 * clean tab close and recovery-discard, adopted by a restored tab after a
 * crash, and orphans of cleanly-exited sessions are swept at boot. All
 * scratch mutations are serialized per tab through {@link withScratchLock}
 * so an absorb from an opened file can never race the worker-segment scan.
 *
 * Lifecycle mirrors `state/autosave.svelte.ts`: a process-level singleton
 * started once from `ensureProcessInit`, watching the shell reactively so
 * the shell itself stays ignorant of recording.
 */
import { unzipSync } from 'fflate';
import { config } from '../config/store.svelte';
import { shell } from '../multi_tab/shell.svelte';
import { storage } from '../storage';
import { toast } from '../state/toast.svelte';
import { sessionId } from '../state/recoverySession';
import type { DarklyInstance } from '../state/app.svelte';
import { negotiateCodec, type NegotiatedCodec } from './codec';
import {
    RECORDING_FPS,
    RECORDING_VERSION,
    SCRATCH_ROOT,
    scratchDir,
    scratchKey,
    scratchManifestPath,
    scratchSegmentBinPath,
    scratchSegmentJsonPath,
    segmentNumberFromName,
    sessionIdOfScratchKey,
    ZIP_DIR,
    type RecordingManifest,
    type SegmentMeta,
} from './segments';
import type { WorkerInMsg, WorkerOutMsg } from './worker';

/** Config snapshot a recorder was last applied with, for change detection. */
interface AppliedConfig {
    enabled: boolean;
    minIntervalSeconds: number;
    maxLongEdge: number;
}

function readConfig(): AppliedConfig {
    return {
        enabled: config.get('recording.enabled') as boolean,
        minIntervalSeconds: config.get('recording.minIntervalSeconds') as number,
        maxLongEdge: parseInt(config.get('recording.maxLongEdge') as string, 10),
    };
}

// ---------------------------------------------------------------------------
// Per-tab scratch serialization
// ---------------------------------------------------------------------------

const scratchLocks = new Map<string, Promise<unknown>>();

/** Run `fn` exclusively against a scratch dir. FIFO per key — an absorb
 *  enqueued at open time is guaranteed to complete before the recorder's
 *  segment scan enqueued at attach time. */
function withScratchLock<T>(key: string, fn: () => Promise<T>): Promise<T> {
    const prev = scratchLocks.get(key) ?? Promise.resolve();
    const next = prev.then(fn, fn);
    scratchLocks.set(key, next.then(
        () => undefined,
        () => undefined,
    ));
    return next;
}

/** This session's scratch dir name for a tab. */
function keyFor(inst: DarklyInstance): string {
    return scratchKey(sessionId, inst.recoveryId);
}

/** The next free segment number in a tab's scratch (1-based). */
async function nextSegmentNumber(key: string): Promise<number> {
    const entries = await storage.list(scratchDir(key));
    let max = 0;
    for (const e of entries) {
        const n = segmentNumberFromName(e.name);
        if (n !== null && n > max) max = n;
    }
    return max + 1;
}

// ---------------------------------------------------------------------------
// Per-tab recorder
// ---------------------------------------------------------------------------

/** Frames to pump waiting for the forced final capture's readback on stop
 *  before giving up (~0.5s at 60fps). A single capture round-trips in ~2
 *  frames; the ceiling only guards a gated or wedged readback from hanging
 *  teardown. */
const FINAL_CAPTURE_MAX_FRAMES = 30;

class TabRecorder {
    worker: Worker | null = null;
    workerReady = false;
    active = false;
    /** Worker hit its retry limit — capture stays off until the user
     *  toggles the setting (which rebuilds the recorder). */
    sessionDisabled = false;
    negotiated: NegotiatedCodec | null = null;
    applied: AppliedConfig | null = null;
    /** Document canvas dims the codec was negotiated against. A poll
     *  reporting a different canvas *aspect ratio* rolls a new segment. */
    baseDims: { width: number; height: number } | null = null;
    /** True while an aspect-change roll is queued on the busy chain, so a
     *  burst of polls schedules it once. */
    rollQueued = false;
    pollInFlight = false;
    /** Serializes activate/deactivate/roll so a config-change burst can't
     *  interleave two worker lifecycles. */
    busy: Promise<void> = Promise.resolve();
    private flushWaiters: Array<() => void> = [];

    constructor(readonly inst: DarklyInstance) {}

    /** Chain an exclusive lifecycle step. */
    run(step: () => Promise<void>): Promise<void> {
        this.busy = this.busy.then(step, step);
        return this.busy;
    }

    async activate(cfg: AppliedConfig): Promise<void> {
        const engine = this.inst.engine;
        if (!engine || this.active || this.sessionDisabled) return;
        const dims = await engine.api.canvasDimensions();
        const negotiated = await negotiateCodec({
            docWidth: dims.width,
            docHeight: dims.height,
            maxLongEdge: cfg.maxLongEdge,
            fps: RECORDING_FPS,
        });
        if (!negotiated) {
            console.warn('[recording] no supported encoder config — capture disabled');
            this.sessionDisabled = true;
            return;
        }

        const key = keyFor(this.inst);
        const segmentN = await withScratchLock(key, async () => {
            const manifest: RecordingManifest = { version: RECORDING_VERSION, fps: RECORDING_FPS };
            await storage.write(
                scratchManifestPath(key),
                new TextEncoder().encode(JSON.stringify(manifest)),
            );
            return nextSegmentNumber(key);
        });

        const worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
        worker.onmessage = (e: MessageEvent<WorkerOutMsg>) => this.onWorkerMessage(e.data);
        this.worker = worker;
        this.post({
            type: 'init',
            scratchKey: key,
            segmentN,
            codec: negotiated.codec,
            width: negotiated.width,
            height: negotiated.height,
            canvasWidth: dims.width,
            canvasHeight: dims.height,
            bitrate: negotiated.bitrate,
            fps: negotiated.fps,
        });

        this.negotiated = negotiated;
        this.applied = cfg;
        this.baseDims = dims;
        this.active = true;
        engine.api.setRecordingParams({
            enabled: true,
            minIntervalSecs: cfg.minIntervalSeconds,
            width: negotiated.width,
            height: negotiated.height,
            baseWidth: dims.width,
            baseHeight: dims.height,
        });
    }

    /** Finalize the current segment and start a fresh one — a re-negotiation
     *  against the live document (resolution setting change, canvas
     *  aspect-ratio change). */
    async roll(cfg: AppliedConfig): Promise<void> {
        await this.deactivate(true);
        await this.activate(cfg);
    }

    /** Stop capture and finalize the current segment. `engineAlive` is
     *  false when the tab is being closed (its WASM handle is already
     *  freed and must not be touched). */
    async deactivate(engineAlive: boolean): Promise<void> {
        if (engineAlive && this.inst.engine && this.active) {
            // Record the final canvas state before capture stops. The last
            // live-void frame — and any state reached after the last document
            // revision bump — never triggered a capture, so without this the
            // recording would end on a stale frame. Clearing `active` first
            // hands the sole drain to `captureFinalFrame` (the render loop's
            // `pollFrame` now no-ops); it must run before disabling, which
            // clears the completed-frame queue.
            this.active = false;
            await this.captureFinalFrame();
            this.inst.engine.api.setRecordingParams({
                enabled: false,
                minIntervalSecs: 0,
                width: 0,
                height: 0,
                baseWidth: 0,
                baseHeight: 0,
            });
        }
        this.active = false;
        this.workerReady = false;
        this.negotiated = null;
        this.baseDims = null;
        const worker = this.worker;
        this.worker = null;
        if (worker) {
            await new Promise<void>((resolve) => {
                const timeout = setTimeout(() => {
                    worker.terminate();
                    resolve();
                }, 5000);
                worker.addEventListener('message', (e: MessageEvent<WorkerOutMsg>) => {
                    if (e.data.type === 'closed') {
                        clearTimeout(timeout);
                        resolve();
                    }
                });
                worker.postMessage({ type: 'close' } satisfies WorkerInMsg);
            });
        }
        this.settleFlushWaiters();
    }

    /** Force one last capture and drain it to the worker before the recorder
     *  is disabled, so the recording ends on the true final canvas state.
     *  Bounded: a gated capture (e.g. the canvas aspect diverged from the
     *  negotiated base, which holds capture) or a wedged readback yields no
     *  final frame rather than blocking teardown. Each iteration nudges the
     *  render loop so the forced capture is submitted and its readback
     *  completes; `pollFrame` no-ops here (`active` is already false), leaving
     *  this the sole drainer. */
    private async captureFinalFrame(): Promise<void> {
        const engine = this.inst.engine;
        if (!engine || !this.worker || !this.workerReady || !this.negotiated) return;
        engine.api.requestRecordingCapture();
        for (let i = 0; i < FINAL_CAPTURE_MAX_FRAMES; i++) {
            this.inst.requestFrame();
            await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
            const res = await engine.api.pollRecordingFrame();
            if (postFrameToWorker(this, res)) return;
        }
    }

    /** Drain the encoder and persist the segment meta, so the scratch on
     *  disk fully describes every captured frame. No-op when inactive. */
    flush(): Promise<void> {
        if (!this.worker || !this.workerReady) return Promise.resolve();
        return new Promise<void>((resolve) => {
            this.flushWaiters.push(resolve);
            this.post({ type: 'flush' });
        });
    }

    post(msg: WorkerInMsg, transfer?: Transferable[]): void {
        this.worker?.postMessage(msg, transfer ?? []);
    }

    private onWorkerMessage(msg: WorkerOutMsg): void {
        switch (msg.type) {
            case 'ready':
                this.workerReady = true;
                break;
            case 'flushed':
                this.flushWaiters.shift()?.();
                break;
            case 'closed':
                break;
            case 'disabled':
                console.warn('[recording] worker disabled:', msg.reason);
                this.sessionDisabled = true;
                toast.show('warning', 'Process recording stopped — encoding failed.');
                void this.run(() => this.deactivate(true));
                break;
        }
    }

    private settleFlushWaiters(): void {
        for (const w of this.flushWaiters.splice(0)) w();
    }
}

// ---------------------------------------------------------------------------
// Process-level service
// ---------------------------------------------------------------------------

let firstCaptureToastShown = false;

/** The `poll_recording_frame` response shape: the live canvas dims (the
 *  resize signal) plus the drained capture, if any, with its RGBA on the
 *  binary side-channel. */
type RecordingFramePoll = {
    canvasWidth: number;
    canvasHeight: number;
    frame: { width: number; height: number; frameIndex: number } | null;
    bytes?: Uint8Array;
};

/** Copy a drained capture out of the WASM heap and hand it to the encoder
 *  worker, moving the buffer with zero further copies (`ImageData` / worker
 *  transfer reject SharedArrayBuffer-backed views). Frames whose dims don't
 *  match the live segment's negotiated encoder were queued before a roll and
 *  are dropped. Returns whether a frame was posted. Shared by the render-loop
 *  drain and the final-frame drain on stop. */
function postFrameToWorker(rec: TabRecorder, res: RecordingFramePoll): boolean {
    const frame = res.frame;
    if (
        !frame ||
        !res.bytes ||
        !rec.negotiated ||
        frame.width !== rec.negotiated.width ||
        frame.height !== rec.negotiated.height
    ) {
        return false;
    }
    const copy = new Uint8Array(res.bytes.length);
    copy.set(res.bytes);
    rec.post(
        {
            type: 'frame',
            data: copy.buffer,
            frameIndex: frame.frameIndex,
            timestampUs: Date.now() * 1000,
        },
        [copy.buffer],
    );
    if (!firstCaptureToastShown) {
        firstCaptureToastShown = true;
        toast.show('info', 'Process recording is on — Settings → Recording');
    }
    return true;
}

class ProcessRecordingService {
    private tabs = new Map<DarklyInstance, TabRecorder>();
    private stopWatch: (() => void) | null = null;
    private started = false;

    /** Wire the service to config changes + tab lifecycle. Idempotent. */
    start(): void {
        if (this.started) return;
        this.started = true;
        config.onChange(() => this.reconfigure());

        this.stopWatch = $effect.root(() => {
            $effect(() => {
                const live = new Set<DarklyInstance>();
                for (const inst of shell.instances) {
                    if (!inst.engine) continue;
                    live.add(inst);
                    if (!this.tabs.has(inst)) {
                        const rec = new TabRecorder(inst);
                        this.tabs.set(inst, rec);
                        this.apply(rec);
                    }
                }
                for (const [inst, rec] of this.tabs) {
                    if (!live.has(inst)) {
                        this.tabs.delete(inst);
                        void rec.run(() => rec.deactivate(false));
                    }
                }
            });
        });
    }

    /** Tear down (tests / HMR). */
    stop(): void {
        this.stopWatch?.();
        this.stopWatch = null;
        for (const [inst, rec] of this.tabs) {
            this.tabs.delete(inst);
            void rec.run(() => rec.deactivate(true));
        }
        this.started = false;
    }

    /** Re-apply config to every tab: toggle off/on, retune the capture
     *  interval, or roll to a new segment on a resolution change (new dims
     *  force a fresh encoder anyway). */
    private reconfigure(): void {
        for (const rec of this.tabs.values()) this.apply(rec);
    }

    private apply(rec: TabRecorder): void {
        const cfg = readConfig();
        void rec.run(async () => {
            if (!cfg.enabled) {
                if (rec.active) await rec.deactivate(true);
                // An explicit toggle is a fresh user request — clear any
                // per-session failure latch so re-enabling retries.
                rec.sessionDisabled = false;
                rec.applied = cfg;
                return;
            }
            if (rec.active && rec.applied) {
                if (rec.applied.maxLongEdge !== cfg.maxLongEdge) {
                    await rec.roll(cfg);
                } else if (rec.applied.minIntervalSeconds !== cfg.minIntervalSeconds) {
                    rec.applied = cfg;
                    const n = rec.negotiated;
                    if (n && rec.baseDims) {
                        rec.inst.engine?.api.setRecordingParams({
                            enabled: true,
                            minIntervalSecs: cfg.minIntervalSeconds,
                            width: n.width,
                            height: n.height,
                            baseWidth: rec.baseDims.width,
                            baseHeight: rec.baseDims.height,
                        });
                    }
                }
                return;
            }
            await rec.activate(cfg);
        });
    }

    /**
     * Per-frame drain, called from the tab's render loop. Only does work
     * while this tab's recorder is live (poll gating); at most one poll in
     * flight. Frame bytes are copied out of the WASM heap (`ImageData`/
     * worker transfer reject SharedArrayBuffer-backed views) and moved to
     * the worker with zero further copies.
     *
     * Every response also carries the live canvas dims — the poll doubles
     * as the resize signal: a canvas whose aspect ratio has diverged from
     * the negotiated base rolls a new segment at the new aspect (the
     * engine holds capture in the meantime, so no letterboxed frames are
     * ever encoded).
     */
    pollFrame(inst: DarklyInstance): void {
        const rec = this.tabs.get(inst);
        if (!rec || !rec.active || !rec.workerReady || rec.pollInFlight) return;
        const engine = inst.engine;
        if (!engine) return;
        rec.pollInFlight = true;
        engine.api
            .pollRecordingFrame()
            .then((res) => {
                rec.pollInFlight = false;
                if (!rec.active || !rec.worker) return;
                postFrameToWorker(rec, res);
                const base = rec.baseDims;
                if (
                    base &&
                    !rec.rollQueued &&
                    res.canvasWidth * base.height !== res.canvasHeight * base.width
                ) {
                    rec.rollQueued = true;
                    void rec.run(async () => {
                        rec.rollQueued = false;
                        if (rec.active) await rec.roll(rec.applied ?? readConfig());
                    });
                }
            })
            .catch(() => {
                rec.pollInFlight = false;
            });
    }

    /**
     * Seed a tab's scratch from an opened `.darkly`'s embedded recording.
     * Call as soon as the tab exists (before its engine finishes booting) —
     * the FIFO scratch lock then orders this ahead of the recorder's
     * segment scan, so the new session appends after the absorbed segments.
     */
    absorbDarkly(inst: DarklyInstance, zipBytes: Uint8Array): Promise<void> {
        const key = keyFor(inst);
        return withScratchLock(key, async () => {
            let entries: Record<string, Uint8Array>;
            try {
                entries = unzipSync(zipBytes, {
                    filter: (f) => f.name.startsWith(`${ZIP_DIR}/`),
                });
            } catch {
                return; // corrupt zip — the document loader surfaces the error
            }
            for (const [name, bytes] of Object.entries(entries)) {
                const leaf = name.slice(ZIP_DIR.length + 1);
                if (!leaf) continue;
                await storage.write(`${scratchDir(key)}/${leaf}`, bytes);
            }
        });
    }

    /** Move a crashed tab's scratch onto a restored tab's fresh identity
     *  (OPFS has no rename — copy + delete). Call right after the restored
     *  tab is opened, for the same ordering reason as `absorbDarkly`. */
    adoptScratch(
        crashed: { sessionId: string; recoveryId: string },
        inst: DarklyInstance,
    ): Promise<void> {
        const newKey = keyFor(inst);
        return withScratchLock(newKey, async () => {
            const oldDir = scratchDir(scratchKey(crashed.sessionId, crashed.recoveryId));
            for (const entry of await storage.list(oldDir)) {
                if (entry.kind !== 'file') continue;
                const bytes = await storage.read(`${oldDir}/${entry.name}`);
                if (bytes) {
                    await storage.write(`${scratchDir(newKey)}/${entry.name}`, bytes);
                }
            }
            await storage.remove(oldDir);
        });
    }

    /** Drop the scratch of one of this session's own tabs (clean tab
     *  close). Idempotent. */
    clearScratchFor(recoveryId: string): Promise<void> {
        const key = scratchKey(sessionId, recoveryId);
        return withScratchLock(key, () => storage.remove(scratchDir(key)));
    }

    /** Drop a crashed tab's scratch (recovery-discard). Idempotent. */
    discardScratch(crashed: { sessionId: string; recoveryId: string }): Promise<void> {
        const key = scratchKey(crashed.sessionId, crashed.recoveryId);
        return withScratchLock(key, () => storage.remove(scratchDir(key)));
    }

    /**
     * Boot sweep: remove scratch dirs owned by neither this session, a
     * crashed session (kept — their tabs may still be restored), nor a
     * live concurrent session — the exact orphan rule the recovery
     * snapshot store applies in `collectRecovery`.
     */
    async gcOrphans(crashed: Set<string>, live: Set<string>): Promise<void> {
        for (const entry of await storage.list(SCRATCH_ROOT)) {
            if (entry.kind !== 'directory') continue;
            const owner = sessionIdOfScratchKey(entry.name);
            const keep =
                owner !== null &&
                (owner === sessionId || crashed.has(owner) || live.has(owner));
            if (!keep) {
                await storage.remove(`${SCRATCH_ROOT}/${entry.name}`).catch(() => {});
            }
        }
    }

    /**
     * Flush the live segment and return the tab's recording as zip entries
     * (`recording/<name>` → bytes) for embedding in a `.darkly` save.
     * Empty when the tab has no recording.
     */
    async collectZipEntries(inst: DarklyInstance): Promise<Array<{ path: string; bytes: Uint8Array }>> {
        const rec = this.tabs.get(inst);
        if (rec) await rec.flush();
        const key = keyFor(inst);
        return withScratchLock(key, async () => {
            const out: Array<{ path: string; bytes: Uint8Array }> = [];
            for (const entry of await storage.list(scratchDir(key))) {
                if (entry.kind !== 'file') continue;
                const bytes = await storage.read(`${scratchDir(key)}/${entry.name}`);
                if (bytes) out.push({ path: `${ZIP_DIR}/${entry.name}`, bytes });
            }
            return out;
        });
    }

    /**
     * Flush the live segment and read the tab's recording as decoded
     * segment metadata + chunk bytes, sorted by segment number — the
     * export pipeline's input. Segments whose meta or bin is missing or
     * unreadable are skipped (e.g. a crash before the first flush).
     */
    async readRecording(
        inst: DarklyInstance,
    ): Promise<Array<{ meta: SegmentMeta; bin: Uint8Array }>> {
        const rec = this.tabs.get(inst);
        if (rec) await rec.flush();
        const key = keyFor(inst);
        return withScratchLock(key, async () => {
            const numbers: number[] = [];
            for (const entry of await storage.list(scratchDir(key))) {
                const n = segmentNumberFromName(entry.name);
                if (n !== null && entry.name.endsWith('.json') && !numbers.includes(n)) {
                    numbers.push(n);
                }
            }
            numbers.sort((a, b) => a - b);
            const out: Array<{ meta: SegmentMeta; bin: Uint8Array }> = [];
            for (const n of numbers) {
                const metaBytes = await storage.read(scratchSegmentJsonPath(key, n));
                const bin = await storage.read(scratchSegmentBinPath(key, n));
                if (!metaBytes || !bin) continue;
                try {
                    const meta = JSON.parse(new TextDecoder().decode(metaBytes)) as SegmentMeta;
                    // Canvas dims are required (aspect grouping); a segment
                    // without them predates the current format and is skipped
                    // (pre-release: discard, never migrate).
                    if (meta.frameCount > 0 && meta.canvasWidth > 0 && meta.canvasHeight > 0) {
                        out.push({ meta, bin });
                    }
                } catch {
                    // Torn meta — skip the segment, keep the rest.
                }
            }
            return out;
        });
    }

    /** Discard the tab's recording entirely and start a fresh one. The
     *  next save simply won't carry `recording/` entries from the past. */
    async deleteRecording(inst: DarklyInstance): Promise<void> {
        const rec = this.tabs.get(inst);
        if (rec) {
            await rec.run(() => rec.deactivate(true));
        }
        await this.clearScratchFor(inst.recoveryId);
        if (rec) this.apply(rec);
    }

    /** Flush the live segment so the scratch on disk is complete — used by
     *  the export flow before it reads the segments. */
    async flushFor(inst: DarklyInstance): Promise<void> {
        await this.tabs.get(inst)?.flush();
    }
}

export const processRecording = new ProcessRecordingService();

if (import.meta.hot) {
    import.meta.hot.accept(() => import.meta.hot!.invalidate());
}
