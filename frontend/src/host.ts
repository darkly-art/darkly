/**
 * What the desktop host (the Electron preload, desktop/src/preload.ts)
 * injects as `window.electronAPI`. Absent in the web build.
 *
 * Each surface belongs to the subsystem that uses it; this file only
 * composes them.
 */
import type { ElectronStorageBridge } from './storage/types';
import type { GpuReport, HostVerdict } from './gpu';

/** The renderer reports its WebGPU adapter check; the host, which owns the
 *  Chromium launch flags, decides what happens next. */
export interface ElectronGpuBridge {
    report(report: GpuReport): Promise<HostVerdict>;
}

export interface ElectronApi {
    storage: ElectronStorageBridge;
    gpu: ElectronGpuBridge;
}

declare global {
    interface Window {
        electronAPI?: ElectronApi;
    }
}
