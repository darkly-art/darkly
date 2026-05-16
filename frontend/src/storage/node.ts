/**
 * NodeFsStorage — adapter over a host-injected bridge.
 *
 * In the desktop bundle, the Electron preload exposes `window.electronAPI.storage`
 * with the shape declared in types.ts. The host maps each call to a real
 * filesystem operation under the platform's userData directory. The renderer
 * never touches `fs`, `path`, or `process` directly.
 */
import type { DarklyStorage, DirEntry, ElectronStorageBridge } from './types';

export class NodeFsStorage implements DarklyStorage {
    readonly #bridge: ElectronStorageBridge;

    /** Bridge can be injected for testing; defaults to window.electronAPI.storage. */
    constructor(bridge?: ElectronStorageBridge) {
        if (bridge) {
            this.#bridge = bridge;
        } else if (typeof window !== 'undefined' && window.electronAPI?.storage) {
            this.#bridge = window.electronAPI.storage;
        } else {
            throw new Error('NodeFsStorage: no host bridge available');
        }
    }

    read(path: string): Promise<Uint8Array | null> {
        return this.#bridge.read(path);
    }

    write(path: string, data: Uint8Array): Promise<void> {
        return this.#bridge.write(path, data);
    }

    list(dir: string): Promise<DirEntry[]> {
        return this.#bridge.list(dir);
    }

    remove(path: string): Promise<void> {
        return this.#bridge.remove(path);
    }

    exists(path: string): Promise<boolean> {
        return this.#bridge.exists(path);
    }
}
