// Browser replay of the stroke replay matrix's recording, the companion of
// `stroke_replay_matrix` for the WebGPU backend: one handle on one canvas,
// a builtin brush with the matrix's overrides, the recording replayed
// through `strokeTo` the way the brush tool issues it, a frame rendered on
// every animation frame, and the queue watched from the JS side. The page
// writes its result to `window.__result` and sets the title to `done`;
// `scripts/bench-drive.mjs` drives it headlessly. Dev server only: it is
// not a Vite build input.
//
// Query parameters: `brush` (builtin name, default `Pencil`), `stab`
// (stabilizer, default 0.6), `radius` (the matrix's base radius column in
// px, default 100), `buildup` (display value for `paint.buildup`, or
// absent to keep the brush's), `pace` (`realtime`, the default, posts
// events at the recorded cadence; `sync` posts one event at a time, renders,
// and waits for the queue, so each event's cost is a throughput number),
// `w` and `h` (document size, default 1920x1080).
import init, { DarklySession } from '../../wasm/pkg/darkly_wasm';
import { Engine } from '../engine/protocol';
import recording from '../../../crates/darkly/tests/fixtures/recorded_curvy_stroke.json';

const q = new URLSearchParams(location.search);
const W = Number(q.get('w') ?? 1920);
const H = Number(q.get('h') ?? 1080);
const STAB = Number(q.get('stab') ?? 0.6);
const RADIUS = Number(q.get('radius') ?? 100);
const BRUSH = q.get('brush') ?? 'Pencil';
const BUILDUP = q.get('buildup');
const PACE = q.get('pace') ?? 'realtime';
/** Mirrors `darkly::brush::DAB_REFERENCE_SIZE`: `size_port = 2 * radius / reference`. */
const DAB_REFERENCE_SIZE = 512;

interface RecordedEvent {
    x: number;
    y: number;
    time_ms: number;
    [key: string]: unknown;
}
interface Recording {
    canvas_width: number;
    canvas_height: number;
    events: RecordedEvent[];
}

const out = document.getElementById('out')!;
const log = (s: string) => {
    out.textContent += s + '\n';
};

// The queue, watched from the JS side: submit-to-completion latency is the
// saturation signal, and the dispatch count checks `dispatches == dabs`.
const gpu = { queue: null as GPUQueue | null, submits: 0, latencies: [] as number[], dispatches: 0, lastDoneAt: 0 };
const origSubmit = GPUQueue.prototype.submit;
GPUQueue.prototype.submit = function (cbs) {
    const t = performance.now();
    origSubmit.call(this, cbs);
    gpu.queue = this;
    gpu.submits++;
    void this.onSubmittedWorkDone().then(() => {
        gpu.latencies.push(performance.now() - t);
        gpu.lastDoneAt = performance.now();
    });
};
const origDispatch = GPUComputePassEncoder.prototype.dispatchWorkgroups;
GPUComputePassEncoder.prototype.dispatchWorkgroups = function (...a) {
    gpu.dispatches++;
    origDispatch.apply(this, a);
};

function stats(xs: number[]) {
    const s = [...xs].sort((a, b) => a - b);
    const at = (p: number) => s[Math.min(s.length - 1, Math.floor(s.length * p))] ?? 0;
    return { n: s.length, p50: +at(0.5).toFixed(2), p95: +at(0.95).toFixed(2), max: +(s[s.length - 1] ?? 0).toFixed(2) };
}

async function main() {
    const canvas = document.getElementById('c') as HTMLCanvasElement;
    canvas.width = W;
    canvas.height = H;
    canvas.style.width = `${W}px`;
    canvas.style.height = `${H}px`;
    await init();
    const session = new DarklySession();
    const handle = await session.createHandle(canvas, W, H, undefined);
    const engine = new Engine(handle);
    const api = engine.api;
    const info = (await navigator.gpu.requestAdapter())?.info;
    const adapter = `${info?.vendor ?? ''} ${info?.architecture ?? ''} ${info?.description ?? ''}`.trim();

    api.resize({ width: W, height: H });
    // One device pixel per CSS pixel, as the native matrix runs, so the
    // stabilizer resamples at the same canvas spacing in both.
    api.setViewTransform({ pan_x: 0, pan_y: 0, zoom: 1, rotation: 0, mirror_h: false, screen_w: W, screen_h: H, dpr: 1 });
    const layerId = await api.addRaster({ anchor: null });
    await api.brushLoad({ name: BRUSH });
    const sizePort = (2 * RADIUS) / DAB_REFERENCE_SIZE;
    const ports: [string, string, number][] = [
        ['brush_settings', 'stabilize', STAB * 100],
        ['brush_settings', 'size', sizePort * 100],
    ];
    if (BUILDUP !== null) ports.push(['paint', 'buildup', Number(BUILDUP)]);
    for (const [node_id, port_name, display_value] of ports) {
        const r = await api.brushSetExposedPort({ node_id, port_name, display_value });
        if (r && 'error' in r) throw new Error(`${node_id}.${port_name}: ${r.error}`);
    }
    log(`${adapter}: ${BRUSH} stab=${STAB} radius=${RADIUS} buildup=${BUILDUP} pace=${PACE} ${W}x${H}`);

    // The frame loop: a render every animation frame, as the editor does.
    const frameIntervals: number[] = [];
    let lastFrame = 0;
    let running = true;
    const frame = (t: number) => {
        if (lastFrame) frameIntervals.push(t - lastFrame);
        lastFrame = t;
        engine.render(t / 1000);
        if (running) requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
    await new Promise((r) => setTimeout(r, 800));

    const rec = recording as Recording;
    const evs = rec.events;
    const scale = [W / rec.canvas_width, H / rec.canvas_height];
    const t0rec = evs[0].time_ms;
    const strokeDuration = evs[evs.length - 1].time_ms - t0rec;
    const op = (ev: RecordedEvent, t0: number) => ({
        ...ev,
        x: ev.x * scale[0],
        y: ev.y * scale[1],
        time_ms: t0 + (ev.time_ms - t0rec),
    });
    frameIntervals.length = 0;
    const before = { submits: gpu.submits, latencies: gpu.latencies.length, dispatches: gpu.dispatches };
    const postCost: number[] = [];
    const eventSyncMs: number[] = [];

    await api.beginStroke({ id: layerId });
    const t0 = performance.now();
    if (PACE === 'sync') {
        // One event at a time: post, drain and render, wait for the GPU.
        running = false;
        await new Promise((r) => setTimeout(r, 50));
        for (const ev of evs) {
            const a = performance.now();
            api.strokeTo({ op: { op: 'brush_stroke', ...op(ev, t0) } as never });
            engine.render(performance.now() / 1000);
            await gpu.queue!.onSubmittedWorkDone();
            eventSyncMs.push(performance.now() - a);
        }
    } else {
        // The recorded cadence: events post from a timer at their offsets,
        // fire-and-forget, as pointer events reach the brush tool.
        let i = 0;
        await new Promise<void>((resolve) => {
            const tick = () => {
                const now = performance.now() - t0;
                while (i < evs.length && evs[i].time_ms - t0rec <= now) {
                    const a = performance.now();
                    api.strokeTo({ op: { op: 'brush_stroke', ...op(evs[i++], t0) } as never });
                    postCost.push(performance.now() - a);
                }
                if (i < evs.length) setTimeout(tick, 1);
                else resolve();
            };
            tick();
        });
    }
    const tEndPosted = performance.now();
    api.endStroke();
    // The queue is drained once no completion has landed for 300 ms.
    await new Promise<void>((resolve) => {
        const check = () => {
            if (performance.now() - gpu.lastDoneAt > 300 && gpu.latencies.length > before.latencies) resolve();
            else setTimeout(check, 20);
        };
        setTimeout(check, 50);
    });
    running = false;

    const result = {
        brush: BRUSH,
        buildup: BUILDUP,
        stab: STAB,
        radius: RADIUS,
        canvas: `${W}x${H}`,
        pace: PACE,
        adapter,
        events: evs.length,
        stroke_duration_ms: +strokeDuration.toFixed(0),
        behind_by_ms: +(gpu.lastDoneAt - t0 - strokeDuration).toFixed(0),
        gpu_done_after_last_post_ms: +(gpu.lastDoneAt - tEndPosted).toFixed(0),
        event_sync_ms: stats(eventSyncMs),
        event_sync_total_ms: +eventSyncMs.reduce((x, y) => x + y, 0).toFixed(0),
        frames: frameIntervals.length,
        frame_interval_ms: stats(frameIntervals),
        long_frames_over_33ms: frameIntervals.filter((x) => x > 33).length,
        strokeTo_post_ms: stats(postCost),
        submit_to_done_ms: stats(gpu.latencies.slice(before.latencies)),
        submits: gpu.submits - before.submits,
        dispatches: gpu.dispatches - before.dispatches,
    };
    log(JSON.stringify(result, null, 2));
    (window as unknown as { __result: unknown }).__result = result;
    document.title = 'done';
}

main().catch((e: unknown) => {
    log(`ERROR ${e instanceof Error ? (e.stack ?? e.message) : String(e)}`);
    (window as unknown as { __result: unknown }).__result = { error: String(e) };
    document.title = 'done';
});
