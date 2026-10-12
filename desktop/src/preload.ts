/**
 * Preload script: runs in a sandboxed context with access to ipcRenderer.
 * Exposes the typed host bridge to the renderer's window.electronAPI.
 *
 * The shape declared here MUST exactly match the ElectronApi interface in
 * frontend/src/host.ts. If you change one, change the other.
 */
import { contextBridge, ipcRenderer } from 'electron';
// Type-only: a sandboxed preload cannot load local modules at runtime.
import type { GpuReport } from './gpu-launch';

contextBridge.exposeInMainWorld('electronAPI', {
    storage: {
        read:   (p: string)                => ipcRenderer.invoke('storage:read', p),
        write:  (p: string, data: Uint8Array) => ipcRenderer.invoke('storage:write', p, data),
        list:   (dir: string)              => ipcRenderer.invoke('storage:list', dir),
        remove: (p: string)                => ipcRenderer.invoke('storage:remove', p),
        exists: (p: string)                => ipcRenderer.invoke('storage:exists', p),
    },
    gpu: {
        report: (report: GpuReport) => ipcRenderer.invoke('gpu:report', report),
    },
});
