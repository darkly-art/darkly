import init, { DarklyHandle } from '../wasm/pkg/darkly_wasm';

let initialized = false;

export async function initEditor(canvas: HTMLCanvasElement): Promise<DarklyHandle> {
    if (!initialized) {
        await init();
        initialized = true;
    }
    const handle = await DarklyHandle.create(canvas);
    return handle;
}
