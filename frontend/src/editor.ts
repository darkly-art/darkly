import init, { DarklyHandle } from "../wasm/pkg/darkly_wasm.js";

let handle: DarklyHandle | null = null;

export async function initEditor(canvas: HTMLCanvasElement): Promise<DarklyHandle> {
    await init();
    handle = await new DarklyHandle(canvas);
    return handle;
}

export function getHandle(): DarklyHandle | null {
    return handle;
}
