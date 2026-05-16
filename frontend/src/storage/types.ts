/**
 * Storage abstraction for the Darkly directory.
 *
 * Two implementations:
 *   - OpfsStorage: backed by the browser's Origin Private File System.
 *     Used in the hosted web build.
 *   - NodeFsStorage: thin shim over a host-provided bridge at
 *     `window.electronAPI.storage`. Used in the desktop bundle, where the
 *     host (Electron main process) maps the bridge to real filesystem ops
 *     under the platform's userData path (e.g. ~/.config/Darkly/).
 *
 * Paths use forward-slash separators and are relative to the Darkly root
 * (which IS the userData directory in Electron, and IS the OPFS root in
 * the browser). Intermediate directories are created on write as needed.
 */

export interface DirEntry {
    name: string;
    kind: 'file' | 'directory';
}

export interface DarklyStorage {
    /** Read raw bytes. Returns null if the file does not exist. */
    read(path: string): Promise<Uint8Array | null>;

    /** Write raw bytes, creating parent directories as needed. */
    write(path: string, data: Uint8Array): Promise<void>;

    /** List entries in a directory. Empty array if directory is missing. */
    list(dir: string): Promise<DirEntry[]>;

    /** Remove a file or directory recursively. Idempotent. */
    remove(path: string): Promise<void>;

    /** Check whether a file or directory exists. */
    exists(path: string): Promise<boolean>;
}

/**
 * Shape that the desktop host (Electron preload) must inject as
 * `window.electronAPI.storage`. The contract is intentionally minimal —
 * just byte-level I/O, listing, and removal. Higher-level helpers
 * (JSON, text, zip export) live on the renderer side, built on these.
 *
 * The preload bridge in the private deploy repo must satisfy this shape
 * exactly. The companion test at __tests__/node.test.ts exercises the
 * NodeFsStorage adapter against an in-memory mock of this bridge — if
 * the shape ever drifts on either side, the test breaks on the public
 * side and the corresponding test in the private repo breaks on the
 * host side.
 */
export interface ElectronStorageBridge {
    read(path: string): Promise<Uint8Array | null>;
    write(path: string, data: Uint8Array): Promise<void>;
    list(dir: string): Promise<DirEntry[]>;
    remove(path: string): Promise<void>;
    exists(path: string): Promise<boolean>;
}

declare global {
    interface Window {
        electronAPI?: {
            storage: ElectronStorageBridge;
        };
    }
}
