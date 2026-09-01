/**
 * The painter's brushes and packs — reactive mirror, and durable store.
 *
 * The engine is the authority on what the library *is*; this module is the
 * frontend's view of it plus the persistence the engine cannot do for itself.
 * Shipped brushes and packs are rebuilt from embedded YAML on every boot and
 * are **never written** — only what the painter creates or imports is stored.
 * The one thing a fresh install writes is the Favorites pack it seeds, which is
 * the painter's from the moment it exists (see `#seedFavorites`).
 *
 * One file per record, no index. The filename is the id and the id never
 * changes, so a rename rewrites one file in place, a delete removes one file,
 * and nothing can be orphaned or left disagreeing with an index — the
 * reasoning `storage/recovery.ts` states for crash snapshots.
 */
import { app } from './app.svelte';
import { jsonDir } from '../storage/jsonStore';
import type { DarklyStorage } from '../storage/types';
import type { BrushInfo, BrushPackInfo } from '../engine/protocol_gen';
import { recentBrushes } from './recents.svelte';
import { newId } from '../lib/id';
import { PALETTE_ROLES, type PackPalette } from '../lib/packPalette';

/** A painter-created brush, as stored. The graph lives in the engine; what we
 *  persist is enough to put it back. */
export interface StoredBrush {
    id: string;
    name: string;
    /** The brush's node graph, as `brushGraphExportYaml` produces it. */
    yaml: string;
}

/** A painter-created pack, as stored. */
export interface StoredPack {
    id: string;
    name: string;
    description: string;
    icon: string;
    palette: PackPalette;
    members: string[];
}

function validBrush(raw: unknown): StoredBrush | null {
    const o = raw as Partial<StoredBrush> | null;
    if (!o || typeof o.id !== 'string' || typeof o.name !== 'string') return null;
    if (typeof o.yaml !== 'string') return null;
    return { id: o.id, name: o.name, yaml: o.yaml };
}

/** Every role present and a string, or nothing. Not defaulted: a record missing
 *  a role is rejected rather than silently blackened, the same rule the engine
 *  applies to a pack file from anywhere else. */
function validPalette(raw: unknown): PackPalette | null {
    const o = raw as Partial<PackPalette> | null;
    if (!o) return null;
    for (const role of PALETTE_ROLES) if (typeof o[role] !== 'string') return null;
    return { chroma: o.chroma!, refraction: o.refraction!, surface: o.surface! };
}

function validPack(raw: unknown): StoredPack | null {
    const o = raw as Partial<StoredPack> | null;
    if (!o || typeof o.id !== 'string' || typeof o.name !== 'string') return null;
    if (typeof o.icon !== 'string') return null;
    const palette = validPalette(o.palette);
    if (!palette) return null;
    const members = Array.isArray(o.members)
        ? o.members.filter((m): m is string => typeof m === 'string')
        : [];
    return {
        id: o.id,
        name: o.name,
        description: typeof o.description === 'string' ? o.description : '',
        icon: o.icon,
        palette,
        members,
    };
}

export class BrushLibraryStore {
    /** Every brush the engine knows about, shipped and painter-created. */
    brushes = $state<BrushInfo[]>([]);
    /** Every pack, in the engine's order: shipped first, painter's after. */
    packs = $state<BrushPackInfo[]>([]);

    readonly #brushDir;
    readonly #packDir;
    /** Ids the painter owns — the ones that get written back. A shipped
     *  brush or pack is regenerated from YAML each boot and must never be
     *  persisted, or deleting it from the shipped set would leave a copy. */
    #ownBrushes = new Set<string>();
    #ownPacks = new Set<string>();
    /** Brush ids the engine had before hydration replayed anything — the
     *  shipped set. Storing one would shadow the YAML it is rebuilt from. */
    #shipped = new Set<string>();

    constructor(storage?: DarklyStorage) {
        this.#brushDir = jsonDir<StoredBrush>('brushes', validBrush, storage);
        this.#packDir = jsonDir<StoredPack>('packs', validPack, storage);
    }

    /** Pull the engine's current library into the reactive mirror. */
    async refresh(): Promise<void> {
        if (!app.engine) return;
        const snap = await app.engine.api.libraryList();
        this.brushes = snap.brushes ?? [];
        this.packs = snap.packs ?? [];
        // A brush the painter deleted must not linger in the recents ring.
        const live = new Set(this.brushes.map(b => b.id));
        recentBrushes.retain(id => live.has(id));
    }

    /** The pack with `id`, if it exists. */
    pack(id: string): BrushPackInfo | undefined {
        return this.packs.find(p => p.id === id);
    }

    /** Packs the painter may export — every one, since exporting reads only. */
    get exportablePacks(): BrushPackInfo[] {
        return this.packs;
    }

    // ---- hydration ----

    /**
     * Replay the painter's stored brushes and packs into the engine.
     *
     * Runs once at boot, not per canvas handle, because the engine's library
     * is process-global. Idempotent: records are replayed **with their stored
     * ids**, so hydrating twice yields the same ids and names rather than
     * accreting `(2)` suffixes the way a re-import would.
     *
     * A record that fails to load is skipped with a warning rather than being
     * fatal — one corrupt file must not cost the painter their library.
     */
    async hydrate(): Promise<void> {
        if (!app.engine) return;
        const api = app.engine.api;

        // Whatever the engine holds before we replay anything is the shipped
        // set, rebuilt from embedded YAML on every boot.
        await this.refresh();
        this.#shipped = new Set(this.brushes.map(b => b.id));

        const storedBrushes = await this.#brushDir.readAll();
        for (const [id, record] of storedBrushes) {
            try {
                // Restoring a brush means installing its graph as the active
                // one and saving it under its stored id.
                await api.brushGraphImportYaml({ yaml: record.yaml });
                await api.brushSave({ id, name: record.name });
                this.#ownBrushes.add(id);
            } catch (e) {
                console.warn(`[brush library] skipping stored brush '${id}'`, e);
            }
        }

        const storedPacks = await this.#packDir.readAll();
        for (const [id, record] of storedPacks) {
            try {
                await api.packCreate({
                    id,
                    name: record.name,
                    description: record.description,
                    icon: record.icon,
                    palette: record.palette,
                });
                this.#ownPacks.add(id);
            } catch (e) {
                console.warn(`[brush library] skipping stored pack '${id}'`, e);
                continue;
            }
            // Members naming a brush that no longer exists are dropped, and
            // the pack rewritten once. The only self-healing path, and it
            // converges: the next boot has nothing left to drop.
            let dropped = false;
            for (const member of record.members) {
                try {
                    await api.packAddBrush({ pack: id, brush: member });
                } catch {
                    dropped = true;
                }
            }
            if (dropped) {
                await this.refresh();
                this.persistPack(id);
            }
        }

        await this.refresh();
        if (storedPacks.size === 0) await this.#seedFavorites();
    }

    /**
     * Give a painter with no packs of their own a Favorites pack to fill.
     *
     * Favorites is not shipped. A shipped pack is rebuilt from embedded YAML
     * on every boot and so cannot hold an edit, which is why shipped packs are
     * locked; Favorites is the painter's list and has to be able to hold one.
     * So it is an ordinary pack they own, created once when there is nothing
     * stored, and from then on it renames, restyles, deletes and persists like
     * any other.
     *
     * Keyed off "the painter has no stored packs" rather than "no pack is
     * named Favorites", so deleting it is a decision that sticks.
     */
    async #seedFavorites(): Promise<void> {
        if (!app.engine) return;
        const id = newId('pack');
        try {
            await app.engine.api.packCreate({
                id,
                name: 'Favorites',
                description: 'The brushes you reach for most.',
                icon: 'fa6-solid:star',
                // Gold, since a star is gold — seated to read on either theme.
                palette: {
                    chroma: '#d9a92b',
                    refraction: '#d8bd5e',
                    surface: '#1e1a10',
                },
            });
        } catch (e) {
            console.warn('[brush library] could not seed Favorites', e);
            return;
        }
        await this.refresh();
        this.persistPack(id);
    }

    // ---- write-through ----

    /** Record `id` as the painter's and write it. Called after any successful
     *  engine mutation that created or changed a pack. */
    persistPack(id: string): void {
        const pack = this.pack(id);
        // Only the painter's packs are stored; a shipped pack comes back from
        // YAML on the next boot and writing it would shadow the shipped one.
        if (!pack || !pack.can_edit_identity) return;
        this.#ownPacks.add(id);
        this.#packDir.write(id, {
            id: pack.id,
            name: pack.name,
            description: pack.description,
            icon: pack.icon,
            palette: pack.palette,
            members: pack.members,
        });
    }

    /**
     * Persist a freshly-imported pack and every brush that arrived with it.
     *
     * An import can bring in brushes the library did not have, and those are
     * the painter's now — without this the pack would come back on reload
     * naming brushes that did not.
     *
     * Brushes the import *reused* are already stored (if painter-owned) or
     * come back from shipped YAML (if not), so only genuinely new ones need
     * writing. Call after `refresh()`.
     */
    async persistImported(packId: string): Promise<void> {
        const pack = this.pack(packId);
        if (!pack || !app.engine) return;

        for (const member of pack.members) {
            // A brush already stored, or one that ships with the app and comes
            // back from YAML each boot, needs nothing: storing a copy of a
            // shipped brush would shadow the shipped one.
            if (this.#ownBrushes.has(member) || this.#shipped.has(member)) continue;
            const brush = this.brushes.find(b => b.id === member);
            if (brush) await this.persistBrush(member, brush.name);
        }
        this.persistPack(packId);
    }

    /** Persist a brush's graph under its id, read without disturbing whatever
     *  the painter currently has loaded. */
    async persistBrush(id: string, name: string): Promise<void> {
        if (!app.engine) return;
        try {
            const yaml = await app.engine.api.brushExportYaml({ id });
            this.#ownBrushes.add(id);
            this.#brushDir.write(id, { id, name, yaml });
        } catch (e) {
            console.warn(`[brush library] could not persist brush '${id}'`, e);
        }
    }

    /** Rewrite a brush's stored record after a rename. No pack is touched —
     *  membership is id-keyed. */
    async renameBrush(id: string, name: string): Promise<void> {
        if (!app.engine) return;
        await app.engine.api.brushRename({ id, name });
        await this.refresh();
        const stored = (await this.#brushDir.readAll()).get(id);
        if (stored) this.#brushDir.write(id, { ...stored, name });
    }

    /** Delete a brush, its stored record, and its membership everywhere. */
    async deleteBrush(id: string): Promise<void> {
        if (!app.engine) return;
        await app.engine.api.brushDelete({ id });
        await this.#brushDir.remove(id);
        this.#ownBrushes.delete(id);
        await this.refresh();
        // Packs that held it changed, so their records are now stale.
        for (const packId of this.#ownPacks) this.persistPack(packId);
    }

    /** Delete a pack and its stored record. Its brushes survive. */
    async deletePack(id: string): Promise<void> {
        if (!app.engine) return;
        await app.engine.api.packDelete({ id });
        await this.#packDir.remove(id);
        this.#ownPacks.delete(id);
        await this.refresh();
    }

    /** Write anything pending immediately — for `beforeunload`. */
    async flush(): Promise<void> {
        await Promise.all([this.#brushDir.flush(), this.#packDir.flush()]);
    }
}

export const brushLibrary = new BrushLibraryStore();
