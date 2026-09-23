/**
 * Preload script: runs in a sandboxed context with access to ipcRenderer.
 * Exposes the typed storage bridge to the renderer's window.electronAPI.
 *
 * The shape declared here MUST exactly match the ElectronStorageBridge
 * interface in the public repo at frontend/src/storage/types.ts. If you
 * change one, change the other.
 */
import { contextBridge, ipcRenderer } from 'electron';

contextBridge.exposeInMainWorld('electronAPI', {
    storage: {
        read:   (p: string)                => ipcRenderer.invoke('storage:read', p),
        write:  (p: string, data: Uint8Array) => ipcRenderer.invoke('storage:write', p, data),
        list:   (dir: string)              => ipcRenderer.invoke('storage:list', dir),
        remove: (p: string)                => ipcRenderer.invoke('storage:remove', p),
        exists: (p: string)                => ipcRenderer.invoke('storage:exists', p),
    },
});
