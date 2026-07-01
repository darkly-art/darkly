<script lang="ts">
    import Modal from '../Modal.svelte';
    import { fontLibrary } from '../../state/font_library.svelte';
    import { loadCatalog, previewUrl, importFont, type CatalogFont } from '../../lib/google_fonts';
    import { toast } from '../../state/toast.svelte';

    interface Props {
        open: boolean;
        onSelect: (family: string) => void;
        onClose: () => void;
    }
    let { open = $bindable(false), onSelect, onClose }: Props = $props();

    let query = $state('');
    /** Custom preview text. Empty → each tile previews its own family name. */
    let preview = $state('');
    let catalog = $state<CatalogFont[]>([]);
    let catalogLoaded = $state(false);
    /** Families currently being imported from Google (shows a spinner + guards
     *  double-clicks). */
    let importing = $state<Set<string>>(new Set());
    let dragOver = $state(false);

    /** How many Google results to render — previews load a network font each, so
     *  we cap and lean on search to narrow. */
    const GOOGLE_LIMIT = 60;

    // Load the catalog only once the browser is actually opened — it's hundreds
    // of KB most sessions never need.
    $effect(() => {
        if (open && !catalogLoaded) {
            catalogLoaded = true;
            loadCatalog()
                .then((c) => {
                    catalog = c;
                })
                .catch((e) => {
                    console.error('[fonts] catalog load failed', e);
                    toast.show('error', 'Failed to load the Google Fonts catalog');
                });
        }
    });

    function tokens(q: string): string[] {
        return q.toLowerCase().trim().split(/\s+/).filter((t) => t.length > 0);
    }

    function matches(family: string, category: string, q: string): boolean {
        if (!q) return true;
        const haystack = `${family} ${category}`.toLowerCase();
        return tokens(q).every((t) => haystack.includes(t));
    }

    const installed = $derived(fontLibrary.families.filter((f) => matches(f, '', query)));

    // Catalog fonts already in the library are shown under Installed, not Google.
    const installedSet = $derived(new Set(fontLibrary.families));
    const googleResults = $derived(
        catalog
            .filter((f) => !installedSet.has(f.family) && matches(f.family, f.category, query))
            .slice(0, GOOGLE_LIMIT),
    );

    /** What a tile renders in its own face: the user's custom preview text, or
     *  the family's own name when the override is empty. */
    function sampleFor(family: string): string {
        const custom = preview.trim();
        return custom.length ? preview : family;
    }

    /** Preview stylesheet URLs — one keyless css2 embed per visible Google font
     *  so each tile renders in its own face. Loads the full Latin subset (not a
     *  per-tile `&text=` subset, which hits a CORS-flaky endpoint), so the
     *  custom preview override re-renders from already-loaded glyphs without
     *  refetching. Re-derives only when the visible set changes. */
    const previewLinks = $derived(googleResults.map((f) => previewUrl(f)));

    async function pickGoogle(font: CatalogFont): Promise<void> {
        if (importing.has(font.family)) return;
        importing = new Set(importing).add(font.family);
        try {
            const families = await importFont(font);
            if (families.length === 0) {
                toast.show('error', `Couldn't import ${font.family}`);
                return;
            }
            onSelect(families[0]);
            onClose();
        } catch (e) {
            console.error('[fonts] import failed', e);
            toast.show('error', `Couldn't import ${font.family}`);
        } finally {
            const next = new Set(importing);
            next.delete(font.family);
            importing = next;
        }
    }

    function pickInstalled(family: string): void {
        onSelect(family);
        onClose();
    }

    async function ingestFile(file: File): Promise<void> {
        const name = file.name.toLowerCase();
        if (!name.endsWith('.ttf') && !name.endsWith('.otf')) {
            toast.show('error', 'Only .ttf and .otf fonts are supported');
            return;
        }
        const bytes = new Uint8Array(await file.arrayBuffer());
        const families = await fontLibrary.add(bytes, 'upload');
        if (families.length === 0) {
            toast.show('error', `Couldn't read ${file.name}`);
            return;
        }
        onSelect(families[0]);
        onClose();
    }

    function onDrop(e: DragEvent): void {
        e.preventDefault();
        dragOver = false;
        const file = e.dataTransfer?.files[0];
        if (file) void ingestFile(file);
    }

    function onPick(e: Event): void {
        const input = e.currentTarget as HTMLInputElement;
        const file = input.files?.[0];
        if (file) void ingestFile(file);
    }
</script>

<svelte:head>
    {#each previewLinks as href (href)}
        <link rel="stylesheet" {href} />
    {/each}
</svelte:head>

<Modal bind:open title="Fonts" size="lg">
    <div class="font-browser">
        <div class="fields">
            <input
                class="search"
                type="search"
                placeholder="Search fonts…"
                bind:value={query}
                autocomplete="off"
                spellcheck="false"
            />
            <input
                class="search preview-input"
                type="text"
                placeholder="Preview text (defaults to the font name)…"
                bind:value={preview}
                autocomplete="off"
                spellcheck="false"
            />
        </div>

        <div
            class="dropzone"
            class:dragOver
            ondrop={onDrop}
            ondragover={(e) => {
                e.preventDefault();
                dragOver = true;
            }}
            ondragleave={() => (dragOver = false)}
            role="button"
            tabindex="0"
        >
            <p>Drop a <code>.ttf</code> / <code>.otf</code> here, or</p>
            <label class="pick">
                upload a font
                <input type="file" accept=".ttf,.otf" onchange={onPick} />
            </label>
        </div>

        <div class="scroll">
            {#if installed.length}
                <h3>Installed</h3>
                <div class="grid">
                    {#each installed as family (family)}
                        <button
                            class="tile"
                            style="font-family: '{family}', sans-serif"
                            onclick={() => pickInstalled(family)}
                            title={family}
                        >
                            <span class="sample">{sampleFor(family)}</span>
                            <span class="name">{family}</span>
                        </button>
                    {/each}
                </div>
            {/if}

            <h3>Google Fonts</h3>
            {#if !catalog.length}
                <p class="muted">Loading catalog…</p>
            {:else if !googleResults.length}
                <p class="muted">No matches.</p>
            {:else}
                <div class="grid">
                    {#each googleResults as font (font.family)}
                        <button
                            class="tile"
                            style="font-family: '{font.family}', sans-serif"
                            onclick={() => pickGoogle(font)}
                            disabled={importing.has(font.family)}
                            title={font.family}
                        >
                            <span class="sample">{sampleFor(font.family)}</span>
                            <span class="name">
                                {font.family}
                                {#if importing.has(font.family)}<span class="spinner">…</span>{/if}
                            </span>
                        </button>
                    {/each}
                </div>
            {/if}
        </div>
    </div>
</Modal>

<style>
    .font-browser {
        display: flex;
        flex-direction: column;
        gap: 12px;
        min-width: 520px;
        max-height: 70vh;
    }

    .fields {
        display: flex;
        gap: 8px;
    }
    .fields .search {
        flex: 1;
        min-width: 0;
    }

    .search {
        width: 100%;
        box-sizing: border-box;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        color: var(--text);
        font-size: 14px;
        padding: 8px 10px;
        outline: none;
    }
    .search:focus {
        border-color: var(--accent);
    }

    .dropzone {
        border: 1px dashed var(--border, var(--bg-hover));
        border-radius: var(--radius-sm);
        padding: 12px;
        text-align: center;
        color: var(--text-muted);
        font-size: 13px;
    }
    .dropzone.dragOver {
        border-color: var(--accent);
        color: var(--text);
    }
    .dropzone code {
        background: var(--bg-hover);
        padding: 1px 4px;
        border-radius: 3px;
    }
    .pick {
        color: var(--accent);
        cursor: pointer;
        text-decoration: underline;
    }
    .pick input {
        display: none;
    }

    .scroll {
        overflow-y: auto;
        flex: 1;
    }

    h3 {
        font-size: 12px;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--text-muted);
        margin: 12px 0 6px;
    }

    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
        gap: 8px;
    }

    .tile {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 4px;
        background: var(--bg-hover);
        border: 1px solid transparent;
        border-radius: var(--radius-sm);
        padding: 10px 12px;
        cursor: pointer;
        text-align: left;
        color: var(--text);
        overflow: hidden;
    }
    .tile:hover {
        border-color: var(--accent);
    }
    .tile:disabled {
        opacity: 0.5;
        cursor: default;
    }
    .sample {
        font-size: 26px;
        line-height: 1.15;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        max-width: 100%;
    }
    .name {
        font-family: var(--font-ui, sans-serif);
        font-size: 12px;
        color: var(--text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        max-width: 100%;
    }
    .spinner {
        color: var(--accent);
    }
    .muted {
        color: var(--text-muted);
        font-size: 13px;
    }
</style>
