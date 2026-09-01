import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { DarklyStorage, DirEntry } from '../../storage/types';
import { app, DarklyInstance, setActiveInstance } from '../app.svelte';
import { BrushLibraryStore } from '../brush_library.svelte';
import type { PackPalette } from '../../lib/packPalette';

/** A shape-valid palette. Which colours a pack wears is not what any of these
 *  tests are about, so there is one fixture rather than four literals a dozen
 *  times over. */
const PALETTE: PackPalette = {
    chroma: '#2f7fe0', refraction: '#2fd0c0', surface: '#0c1a26',
};

/** In-memory storage. */
class FakeStorage implements DarklyStorage {
    files = new Map<string, Uint8Array>();
    async read(path: string) { return this.files.get(path) ?? null; }
    async write(path: string, data: Uint8Array) { this.files.set(path, data); }
    async list(dir: string): Promise<DirEntry[]> {
        const prefix = dir ? `${dir}/` : '';
        const out: DirEntry[] = [];
        for (const p of this.files.keys()) {
            if (!p.startsWith(prefix)) continue;
            const rest = p.slice(prefix.length);
            if (rest.length === 0 || rest.includes('/')) continue;
            out.push({ name: rest, kind: 'file' });
        }
        return out;
    }
    async remove(path: string) { this.files.delete(path); }
    async exists(path: string) { return this.files.has(path); }

    put(path: string, value: unknown) {
        this.files.set(path, new TextEncoder().encode(JSON.stringify(value)));
    }
    putRaw(path: string, text: string) {
        this.files.set(path, new TextEncoder().encode(text));
    }
    json(path: string): Record<string, unknown> | null {
        const b = this.files.get(path);
        return b ? JSON.parse(new TextDecoder().decode(b)) : null;
    }
    paths(prefix: string): string[] {
        return [...this.files.keys()].filter(p => p.startsWith(prefix)).sort();
    }
}

/** A fake engine holding a library the way the real one does: brushes keyed
 *  by id, packs owning member lists. Shipped entries seed it, exactly as the
 *  real engine rebuilds them from YAML each boot. */
function fakeEngine() {
    const brushes = new Map<string, { id: string; name: string }>([
        ['ink_pen', { id: 'ink_pen', name: 'Ink Pen' }],
    ]);
    const packs = new Map<string, {
        id: string; name: string; description: string; icon: string;
        palette: PackPalette; members: string[];
        can_edit_members: boolean; can_edit_identity: boolean;
    }>([
        ['basic', {
            id: 'basic', name: 'Basic', description: '', icon: 'mdi:brush',
            palette: PALETTE, members: ['ink_pen'],
            can_edit_members: false, can_edit_identity: false,
        }],
    ]);

    const api = {
        libraryList: vi.fn(async () => ({
            brushes: [...brushes.values()].map(b => ({ ...b, author: '', description: '', tags: [], icon: null })),
            packs: [...packs.values()].map(p => ({ ...p, members: [...p.members] })),
        })),
        brushGraphImportYaml: vi.fn(async ({ yaml }: { yaml: string }) => {
            if (yaml === 'CORRUPT') throw new Error('bad graph');
            return null;
        }),
        brushSave: vi.fn(async ({ id, name }: { id: string; name: string }) => {
            brushes.set(id, { id, name });
            return null;
        }),
        brushExportYaml: vi.fn(async ({ id }: { id: string }) => `yaml-for-${id}`),
        brushRename: vi.fn(async ({ id, name }: { id: string; name: string }) => {
            const b = brushes.get(id);
            if (b) b.name = name;
            return null;
        }),
        brushDelete: vi.fn(async ({ id }: { id: string }) => {
            brushes.delete(id);
            for (const p of packs.values()) p.members = p.members.filter(m => m !== id);
            return null;
        }),
        packCreate: vi.fn(async (r: {
            id: string; name: string; description: string;
            icon: string; palette: PackPalette;
        }) => {
            if (packs.has(r.id)) throw new Error('duplicate pack id');
            packs.set(r.id, {
                ...r, members: [],
                can_edit_members: true, can_edit_identity: true,
            });
            return null;
        }),
        packAddBrush: vi.fn(async ({ pack, brush }: { pack: string; brush: string }) => {
            const p = packs.get(pack);
            if (!p) throw new Error('no such pack');
            if (!brushes.has(brush)) throw new Error('no such brush');
            if (!p.members.includes(brush)) p.members.push(brush);
            return null;
        }),
        packDelete: vi.fn(async ({ id }: { id: string }) => {
            packs.delete(id);
            return null;
        }),
    };
    return { engine: { api } as unknown as NonNullable<typeof app.engine>, brushes, packs, api };
}

let s: FakeStorage;
let store: BrushLibraryStore;
let fake: ReturnType<typeof fakeEngine>;

beforeEach(() => {
    s = new FakeStorage();
    fake = fakeEngine();
    // `app` is a proxy onto the active instance, so one must exist before
    // `app.engine` can be set.
    setActiveInstance(new DarklyInstance());
    app.engine = fake.engine;
    store = new BrushLibraryStore(s);
});

afterEach(() => {
    setActiveInstance(null);
});

describe('brush library persistence', () => {
    it('a_fresh_install_writes_only_the_seeded_favorites', async () => {
        // Shipped brushes and packs come back from YAML every boot; storing a
        // copy would shadow them. Favorites is the exception because it is not
        // shipped: it is the painter's, created here so they have somewhere to
        // put a brush on the first day.
        await store.hydrate();
        await store.flush();

        const favorites = store.packs.find(p => p.name === 'Favorites')!;
        expect(s.paths('')).toEqual([`packs/${favorites.id}.json`]);
    });

    it('hydrate_imports_every_stored_record', async () => {
        s.put('brushes/b1.json', { id: 'b1', name: 'Mine', yaml: 'nodes: {}' });
        s.put('packs/p1.json', {
            id: 'p1', name: 'My Pack', description: 'd', icon: 'mdi:water',
            palette: PALETTE, members: ['b1'],
        });

        await store.hydrate();

        expect(store.brushes.map(b => b.id).sort()).toEqual(['b1', 'ink_pen']);
        const p1 = store.pack('p1');
        expect(p1?.name).toBe('My Pack');
        expect(p1?.members).toEqual(['b1']);
    });

    it('hydrate_is_idempotent_across_reloads', async () => {
        s.put('brushes/b1.json', { id: 'b1', name: 'Mine', yaml: 'nodes: {}' });
        s.put('packs/p1.json', {
            id: 'p1', name: 'My Pack', description: '', icon: 'mdi:water',
            palette: PALETTE, members: ['b1'],
        });

        await store.hydrate();
        const first = { brushes: store.brushes.length, name: store.pack('p1')?.name };

        // A second boot against the same files, and a fresh engine.
        fake = fakeEngine();
        app.engine = fake.engine;
        const second = new BrushLibraryStore(s);
        await second.hydrate();

        expect(second.brushes.length).toBe(first.brushes);
        // No "(2)" accretion: hydration replays with the stored id, which is
        // not the import-a-stranger's-file path.
        expect(second.pack('p1')?.name).toBe(first.name);
        expect(second.pack('p1')?.name).toBe('My Pack');
    });

    it('a_record_that_fails_to_import_is_skipped_not_fatal', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        s.put('brushes/good.json', { id: 'good', name: 'Good', yaml: 'nodes: {}' });
        s.put('brushes/bad.json', { id: 'bad', name: 'Bad', yaml: 'CORRUPT' });
        s.putRaw('brushes/unreadable.json', 'not json at all');

        await store.hydrate();

        expect(store.brushes.some(b => b.id === 'good')).toBe(true);
        expect(store.brushes.some(b => b.id === 'bad')).toBe(false);
        warn.mockRestore();
    });

    it('a_member_naming_a_missing_brush_is_dropped_on_hydrate', async () => {
        s.put('packs/p1.json', {
            id: 'p1', name: 'My Pack', description: '', icon: 'mdi:water',
            palette: PALETTE, members: ['ink_pen', 'ghost'],
        });

        await store.hydrate();
        await store.flush();

        expect(store.pack('p1')?.members).toEqual(['ink_pen']);
        // Self-healing converges: the rewritten record no longer names it.
        expect(s.json('packs/p1.json')?.members).toEqual(['ink_pen']);
    });

    it('renaming_a_pack_leaves_no_stale_file', async () => {
        s.put('packs/p1.json', {
            id: 'p1', name: 'Before', description: '', icon: 'mdi:water',
            palette: PALETTE, members: [],
        });
        await store.hydrate();

        // Rename in the engine, then write through.
        fake.packs.get('p1')!.name = 'After';
        await store.refresh();
        store.persistPack('p1');
        await store.flush();

        expect(s.paths('packs/')).toEqual(['packs/p1.json']);
        expect(s.json('packs/p1.json')?.name).toBe('After');
    });

    it('deleting_a_pack_removes_its_file_and_no_other', async () => {
        for (const id of ['p1', 'p2']) {
            s.put(`packs/${id}.json`, {
                id, name: id, description: '', icon: 'mdi:water',
                palette: PALETTE, members: [],
            });
        }
        await store.hydrate();

        await store.deletePack('p1');
        await store.flush();

        expect(s.paths('packs/')).toEqual(['packs/p2.json']);
    });

    it('two_packs_with_names_that_sanitize_alike_both_persist', async () => {
        // Ids, not slugs: `"A/B"` and `"A:B"` would collide as filenames.
        await store.hydrate();
        await fake.api.packCreate({
            id: 'id-one', name: 'A/B', description: '', icon: 'mdi:water',
            palette: PALETTE,
        });
        await fake.api.packCreate({
            id: 'id-two', name: 'A:B', description: '', icon: 'mdi:water',
            palette: PALETTE,
        });
        await store.refresh();
        store.persistPack('id-one');
        store.persistPack('id-two');
        await store.flush();

        // Both survive as separate files. The seeded Favorites is also on
        // disk, so this asserts the two in question rather than the whole
        // directory.
        expect(s.paths('packs/')).toEqual(
            expect.arrayContaining(['packs/id-one.json', 'packs/id-two.json']),
        );
        expect(s.json('packs/id-one.json')?.name).toBe('A/B');
        expect(s.json('packs/id-two.json')?.name).toBe('A:B');
    });

    it('a_shipped_pack_is_never_written', async () => {
        await store.hydrate();
        await store.flush();
        const before = s.paths('packs/');

        store.persistPack('basic');
        await store.flush();

        expect(s.paths('packs/')).toEqual(before);
        expect(s.json('packs/basic.json')).toBeNull();
    });

    // ---- Favorites ----

    it('a_brush_added_to_favorites_survives_a_reload', async () => {
        await store.hydrate();
        const favorites = store.packs.find(p => p.name === 'Favorites');
        expect(favorites, 'the painter has a Favorites pack').toBeDefined();

        await fake.api.packAddBrush({ pack: favorites!.id, brush: 'ink_pen' });
        await store.refresh();
        store.persistPack(favorites!.id);
        await store.flush();

        // A second boot against the same files and a fresh engine, exactly as
        // `hydrate_is_idempotent_across_reloads` does.
        fake = fakeEngine();
        app.engine = fake.engine;
        const second = new BrushLibraryStore(s);
        await second.hydrate();

        const reloaded = second.packs.find(p => p.name === 'Favorites');
        expect(reloaded?.members).toEqual(['ink_pen']);
    });

    it('favorites_is_seeded_once_and_not_recreated', async () => {
        await store.hydrate();
        const seeded = store.packs.filter(p => p.name === 'Favorites');
        expect(seeded).toHaveLength(1);

        fake = fakeEngine();
        app.engine = fake.engine;
        const second = new BrushLibraryStore(s);
        await second.hydrate();

        expect(second.packs.filter(p => p.name === 'Favorites')).toHaveLength(1);
    });

    it('a_painter_who_deleted_favorites_does_not_get_it_back', async () => {
        await store.hydrate();
        const favorites = store.packs.find(p => p.name === 'Favorites')!;
        await store.deletePack(favorites.id);
        await store.flush();

        // Storage still holds a pack, so the seed does not fire again.
        s.put('packs/keep.json', {
            id: 'keep', name: 'Keep', description: '', icon: 'mdi:water',
            palette: PALETTE, members: [],
        });
        fake = fakeEngine();
        app.engine = fake.engine;
        const second = new BrushLibraryStore(s);
        await second.hydrate();

        expect(second.packs.find(p => p.name === 'Favorites')).toBeUndefined();
    });

    it('a_stored_pack_missing_a_palette_role_is_not_loaded', async () => {
        // No defaulting and no migration: a record the current shape rejects is
        // skipped rather than silently blackened. Pre-release, invalidating a
        // painter's stored packs is the accepted cost of breaking the format.
        s.files.set(
            'packs/old.json',
            new TextEncoder().encode(JSON.stringify({
                id: 'old', name: 'Old', description: '', icon: 'mdi:brush',
                palette: { chroma: '#2f7fe0', refraction: '#2fd0c0' },
                members: [],
            })),
        );
        await store.hydrate();
        expect(store.packs.find(p => p.id === 'old')).toBeUndefined();
    });

    it('persistPack_writes_every_palette_role', async () => {
        await store.hydrate();
        await fake.api.packCreate({
            id: 'p-new', name: 'Theirs', description: '', icon: 'mdi:water',
            palette: PALETTE,
        });
        await store.refresh();
        store.persistPack('p-new');
        await store.flush();

        expect(s.json('packs/p-new.json')?.palette).toEqual(PALETTE);
    });

    it('deleting_a_brush_removes_its_file_and_rewrites_the_packs_that_held_it', async () => {
        s.put('brushes/b1.json', { id: 'b1', name: 'Mine', yaml: 'nodes: {}' });
        s.put('packs/p1.json', {
            id: 'p1', name: 'My Pack', description: '', icon: 'mdi:water',
            palette: PALETTE, members: ['b1'],
        });
        await store.hydrate();

        await store.deleteBrush('b1');
        await store.flush();

        expect(s.paths('brushes/')).toEqual([]);
        expect(s.json('packs/p1.json')?.members).toEqual([]);
    });

    it('renaming_a_brush_rewrites_its_record_and_touches_no_pack', async () => {
        s.put('brushes/b1.json', { id: 'b1', name: 'Before', yaml: 'nodes: {}' });
        s.put('packs/p1.json', {
            id: 'p1', name: 'My Pack', description: '', icon: 'mdi:water',
            palette: PALETTE, members: ['b1'],
        });
        await store.hydrate();
        await store.flush();
        const packBefore = s.json('packs/p1.json');

        await store.renameBrush('b1', 'After');
        await store.flush();

        expect(s.json('brushes/b1.json')?.name).toBe('After');
        expect(s.json('packs/p1.json')).toEqual(packBefore);
    });

    it('persistImported_stores_new_brushes_but_not_shipped_ones', async () => {
        await store.hydrate();
        // An import brought in one new brush and reused a shipped one.
        await fake.api.brushSave({ id: 'imported', name: 'Imported' });
        await fake.api.packCreate({
            id: 'p-new', name: 'Theirs', description: '', icon: 'mdi:water',
            palette: PALETTE,
        });
        await fake.api.packAddBrush({ pack: 'p-new', brush: 'imported' });
        await fake.api.packAddBrush({ pack: 'p-new', brush: 'ink_pen' });
        await store.refresh();

        await store.persistImported('p-new');
        await store.flush();

        expect(s.paths('brushes/')).toEqual(['brushes/imported.json']);
        expect(s.json('brushes/imported.json')?.yaml).toBe('yaml-for-imported');
        expect(s.json('packs/p-new.json')?.members).toEqual(['imported', 'ink_pen']);
    });
});
