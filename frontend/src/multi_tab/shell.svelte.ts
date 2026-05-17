import { DarklyInstance, setActiveInstance } from '../state/app.svelte';

/**
 * Optional multi-tab layer. Owns a collection of `DarklyInstance`s and
 * tracks which one is currently focused. Each instance is fully
 * self-contained — the shell does not subclass or wrap them; it merely
 * holds a list and tells the global `app` proxy which instance to resolve
 * to via [`setActiveInstance`].
 *
 * Embedded hosts that want a single Darkly instance never load this module
 * — the rest of the app works perfectly with just `DarklyInstance`.
 */
class MultiTabShell {
    /** Open instances, in tab-strip order. */
    instances = $state<DarklyInstance[]>([]);

    /** Stable id of the focused instance, or `null` when no tabs are open. */
    activeId = $state<string | null>(null);

    /** Monotonic counter, bumped whenever any tab's name changes. Reads
     *  in [`nameOf`] depend on it so Svelte re-derives even though the
     *  underlying value lives on the engine (a plain WASM call, opaque
     *  to Svelte's reactivity). */
    private nameVersion = $state(0);

    private nextSerial = 1;

    get active(): DarklyInstance | null {
        if (!this.activeId) return null;
        return this.instances.find(i => i.id === this.activeId) ?? null;
    }

    /** Tab title for `id`. Reads through the engine's `document_name()`
     *  when the handle is ready; falls back to the pending name (set by
     *  `open(name?)` and applied to the engine post-handle-init) or
     *  `"Untitled"` for instances whose handles haven't bootstrapped. */
    nameOf(id: string): string {
        // Subscribe to the version counter so Svelte re-runs on rename.
        void this.nameVersion;
        const inst = this.instances.find(i => i.id === id);
        if (!inst) return 'Untitled';
        if (inst.handle) return inst.handle.document_name();
        return inst.pendingName ?? 'Untitled';
    }

    /** Rename a tab. Persists into the engine via `set_document_name`
     *  (queued — visible on the next render). If the instance's handle
     *  hasn't booted yet, the name is stashed on `pendingName` for the
     *  init path to apply. */
    setName(id: string, name: string): void {
        const inst = this.instances.find(i => i.id === id);
        if (!inst) return;
        if (inst.handle) {
            inst.handle.set_document_name(name);
        } else {
            inst.pendingName = name;
        }
        this.nameVersion++;
    }

    /** Add a fresh, empty `DarklyInstance` to the strip and focus it. The
     *  instance's WASM handle is allocated lazily — it's set up when the
     *  per-tab `<CanvasView {instance}/>` mounts and bootstraps the canvas.
     *  This keeps tab open instant (no await) and matches Svelte's
     *  template-driven canvas creation.
     *
     *  `dims` overrides the global `canvas.width/height` config defaults
     *  for this tab only — used by the Open flow when the source file
     *  has its own intrinsic dimensions (a `.png` opens as a new tab
     *  sized to the image; a `.darkly` ignores this and lets the
     *  loader's internal resize take over). */
    open(name?: string, dims?: { width: number; height: number }): DarklyInstance {
        const inst = new DarklyInstance();
        inst.pendingName = name ?? `Untitled ${this.nextSerial++}`;
        if (dims) inst.pendingDims = dims;
        this.instances.push(inst);
        this.nameVersion++;
        this.focus(inst.id);
        return inst;
    }

    /** Switch focus to `id`. Updates the global `app` proxy so every UI
     *  component that reads `app.<x>` re-runs against the new instance. */
    focus(id: string): void {
        if (!this.instances.some(i => i.id === id)) return;
        this.activeId = id;
        setActiveInstance(this.active);
    }

    /** Move the tab with `id` to position `toIndex` in `instances`.
     *  No-op if the id isn't present, the index is out of range, or the
     *  order wouldn't change. Active tab and names are unaffected — only
     *  the strip order changes. */
    reorder(id: string, toIndex: number): void {
        const fromIndex = this.instances.findIndex(i => i.id === id);
        if (fromIndex === -1) return;
        if (toIndex < 0 || toIndex >= this.instances.length) return;
        if (toIndex === fromIndex) return;
        const [inst] = this.instances.splice(fromIndex, 1);
        this.instances.splice(toIndex, 0, inst);
    }

    /** Close `id`. Drops the instance's WASM handle (and thus the engine
     *  and its GPU textures), focuses the previous tab when the closed one
     *  was active, or null when none remain. */
    close(id: string): void {
        const idx = this.instances.findIndex(i => i.id === id);
        if (idx === -1) return;
        const [removed] = this.instances.splice(idx, 1);

        // Free the WASM handle: drops the Rust DarklyEngine, returning all
        // its GPU textures to the shared device. No effect on sibling
        // instances since the device is `Arc`-shared.
        removed.handle?.free();
        this.nameVersion++;

        if (this.activeId === id) {
            const next = this.instances[idx] ?? this.instances[idx - 1] ?? null;
            this.activeId = next?.id ?? null;
            setActiveInstance(next);
        }
    }
}

export const shell = new MultiTabShell();

if (import.meta.hot) {
    import.meta.hot.accept(() => import.meta.hot!.invalidate());
}
