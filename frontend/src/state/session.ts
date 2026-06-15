import init, { DarklySession } from '../../wasm/pkg/darkly_wasm';
import { Engine } from '../engine/protocol';

/**
 * Process-level WASM bootstrap + shared `DarklySession`.
 *
 * `DarklySession` owns one `wgpu::Instance` and one `Arc<GpuDevice>` —
 * every `DarklyHandle` minted via `session.createHandle(canvas, w, h)` shares
 * the same WebGPU device. Multi-tab editors use one session and N handles,
 * one per open document; the embedded single-instance host uses one session
 * and one handle.
 *
 * `getSession()` is idempotent and lazy: the first caller initialises the
 * WASM module and constructs the session; subsequent callers reuse it. A
 * concurrent first-call returns the same `Promise` — there is exactly one
 * session per process.
 */

let sessionPromise: Promise<DarklySession> | null = null;

async function buildSession(): Promise<DarklySession> {
    await init();
    return new DarklySession();
}

export function getSession(): Promise<DarklySession> {
    if (!sessionPromise) {
        sessionPromise = buildSession();
    }
    return sessionPromise;
}

export async function createHandle(
    canvas: HTMLCanvasElement,
    docWidth: number,
    docHeight: number,
): Promise<Engine> {
    const session = await getSession();
    const handle = await session.createHandle(canvas, docWidth, docHeight);
    return new Engine(handle);
}
