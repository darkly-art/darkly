import { DarklyInstance, setActiveInstance } from '../state/app.svelte';
import { brushGraph } from '../state/brush_graph.svelte';

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

    /** Display name per instance id. The shell is the authoritative mirror
     *  of the engine's document name: `setName` is the single write path
     *  (user rename, Save As, and the post-load sync in `actions/index.ts`
     *  all route through it), and `pendingName` seeds it before init. Reads
     *  go through this reactive map so the tab strip re-renders on rename —
     *  the engine's `document_name` is async and can't back a `$derived`. */
    private names = $state<Record<string, string>>({});

    private nextSerial = 1;

    get active(): DarklyInstance | null {
        if (!this.activeId) return null;
        return this.instances.find(i => i.id === this.activeId) ?? null;
    }

    /** Tab title for `id`. Reads the shell-side mirror (populated by
     *  `setName`), falling back to the pending name (set by `open(name?)`
     *  and applied to the engine post-init) or `"Untitled"` for instances
     *  whose handles haven't bootstrapped. Synchronous — the engine's
     *  `document_name` is async and can't be read inside a `$derived`. */
    nameOf(id: string): string {
        const cached = this.names[id];
        if (cached !== undefined) return cached;
        const inst = this.instances.find(i => i.id === id);
        if (!inst) return 'Untitled';
        return inst.pendingName ?? 'Untitled';
    }

    /** Rename a tab. Persists into the engine via `set_document_name`
     *  (queued — visible on the next render) and updates the shell-side
     *  mirror that `nameOf` reads. If the instance's handle hasn't booted
     *  yet, the name is stashed on `pendingName` for the init path to
     *  apply. */
    setName(id: string, name: string): void {
        const inst = this.instances.find(i => i.id === id);
        if (!inst) return;
        if (inst.engine) {
            inst.engine.api.setDocumentName({ name });
        } else {
            inst.pendingName = name;
        }
        this.names = { ...this.names, [id]: name };
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
        this.focus(inst.id);
        return inst;
    }

    /** Switch focus to `id`. Updates the global `app` proxy so every UI
     *  component that reads `app.<x>` re-runs against the new instance. */
    focus(id: string): void {
        if (!this.instances.some(i => i.id === id)) return;
        this.activeId = id;
        setActiveInstance(this.active);
        // The brushGraph singleton caches the focused engine's graph /
        // exposed ports. Without this resync, preview consumers' $effects
        // (keyed on brushGraph.graph) wouldn't re-fire on tab switch and
        // their previews would freeze until the user picked a brush.
        brushGraph.syncFromActiveEngine();
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

        // Tear the instance down: stops its tool session and stream sources,
        // frees the WASM handle (dropping the Rust DarklyEngine and returning
        // its GPU textures to the shared device), and nulls the engine so any
        // frame still queued on the instance's render loop bails instead of
        // rendering on a freed handle. No effect on sibling instances since the
        // device is `Arc`-shared.
        removed.dispose();
        if (id in this.names) {
            const { [id]: _removed, ...rest } = this.names;
            this.names = rest;
        }

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
