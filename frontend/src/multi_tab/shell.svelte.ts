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

    /** Display name per tab — defaults to "Untitled N". The instance itself
     *  has no `name` field (a Darkly instance shouldn't care what tab strip
     *  it's in), so the shell tracks it externally, keyed by instance id. */
    private names = $state<Record<string, string>>({});

    private nextSerial = 1;

    get active(): DarklyInstance | null {
        if (!this.activeId) return null;
        return this.instances.find(i => i.id === this.activeId) ?? null;
    }

    nameOf(id: string): string {
        return this.names[id] ?? 'Untitled';
    }

    setName(id: string, name: string): void {
        this.names[id] = name;
    }

    /** Add a fresh, empty `DarklyInstance` to the strip and focus it. The
     *  instance's WASM handle is allocated lazily — it's set up when the
     *  per-tab `<CanvasView {instance}/>` mounts and bootstraps the canvas.
     *  This keeps tab open instant (no await) and matches Svelte's
     *  template-driven canvas creation. */
    open(name?: string): DarklyInstance {
        const inst = new DarklyInstance();
        this.names[inst.id] = name ?? `Untitled ${this.nextSerial++}`;
        this.instances.push(inst);
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
        delete this.names[removed.id];

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
