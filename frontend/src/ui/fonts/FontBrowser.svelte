<script lang="ts">
    import Modal from '../Modal.svelte';
    import { fontLibrary } from '../../state/font_library.svelte';
    import { loadCatalog, previewUrl, importFont, type CatalogFont } from '../../lib/google_fonts';
    import { virtualGridWindow } from '../../lib/virtual_grid';
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

    // The Google grid is virtualized (prior art: Graphite's font menu,
    // `frontend/src/components/floating-menus/MenuList.svelte` — virtual
    // scrolling + a lazily-loaded `<link>` per visible entry). Each tile loads a
    // network preview font, and the catalog is 1900+ fonts, so only the rows in
    // (or near) the viewport are put in the DOM: the whole catalog is browsable
    // with a correct scrollbar, but preview fetches and node count stay bounded
    // to the visible window no matter how far you scroll.
    /** Tile footprint — kept in sync with the CSS via the `--tile-h` custom
     *  property set on the container. `MIN_W` matches the grid's min column. */
    const TILE_H = 74;
    const TILE_MIN_W = 150;
    const GAP = 8;
    /** Extra rows rendered above/below the viewport so a fast scroll doesn't
     *  flash empty tiles before their previews load. */
    const ROW_BUFFER = 2;

    let scrollEl = $state<HTMLElement | undefined>(undefined);
    let virtualEl = $state<HTMLElement | undefined>(undefined);
    let scrollTop = $state(0);
    let viewportH = $state(0);
    let gridWidth = $state(0);
    /** Distance from the top of the scroll content to the virtualized grid (the
     *  Installed section + headings sit above it), so scroll math is grid-local. */
    let googleOffsetTop = $state(0);

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
    const googleFiltered = $derived(
        catalog.filter((f) => !installedSet.has(f.family) && matches(f.family, f.category, query)),
    );

    // --- Virtual window over `googleFiltered` --------------------------------
    const win = $derived(
        virtualGridWindow({
            count: googleFiltered.length,
            containerWidth: gridWidth,
            scrollTop,
            offsetTop: googleOffsetTop,
            viewportH,
            tileMinWidth: TILE_MIN_W,
            tileHeight: TILE_H,
            gap: GAP,
            rowBuffer: ROW_BUFFER,
        }),
    );
    /** Only the entries whose rows are in (or near) the viewport are rendered. */
    const googleWindow = $derived(googleFiltered.slice(win.sliceStart, win.sliceEnd));

    /** Sync the scroll/size measurements the virtual window derives from. Cheap
     *  (a few layout reads); called on scroll, on resize, and after the result
     *  set changes (which can move the grid's offset). */
    function measure(): void {
        if (!scrollEl) return;
        scrollTop = scrollEl.scrollTop;
        viewportH = scrollEl.clientHeight;
        if (virtualEl) {
            gridWidth = virtualEl.clientWidth;
            googleOffsetTop = virtualEl.offsetTop;
        }
    }

    // Re-measure after the DOM settles when the result set or catalog changes
    // (the Installed section above the grid may have grown/shrunk, shifting the
    // grid's offset). Reading the lengths registers the reactive dependency.
    $effect(() => {
        void googleFiltered.length;
        void installed.length;
        void open;
        requestAnimationFrame(measure);
    });

    // Keep measurements current as the modal / viewport resizes.
    $effect(() => {
        if (!scrollEl || typeof ResizeObserver === 'undefined') return;
        const ro = new ResizeObserver(() => measure());
        ro.observe(scrollEl);
        return () => ro.disconnect();
    });

    // Narrowing the search jumps back to the top so results start from row 0.
    $effect(() => {
        void query;
        if (scrollEl) scrollEl.scrollTop = 0;
    });

    /** What a tile renders in its own face: the user's custom preview text, or
     *  the family's own name when the override is empty. */
    function sampleFor(family: string): string {
        const custom = preview.trim();
        return custom.length ? preview : family;
    }

    /** Preview stylesheet URLs — one keyless css2 embed per *visible* Google font
     *  so each tile renders in its own face. Virtualization keeps this list small
     *  (only the on-screen window), so preview fetches are bounded no matter how
     *  large the catalog or how far the user scrolls. Loads the full Latin subset
     *  (not a per-tile `&text=` subset, which hits a CORS-flaky endpoint), so the
     *  custom preview override re-renders from already-loaded glyphs. */
    const previewLinks = $derived(googleWindow.map((f) => previewUrl(f)));

    async function pickGoogle(font: CatalogFont): Promise<void> {
        if (importing.has(font.family)) return;
        importing = new Set(importing).add(font.family);
        try {
            const families = await importFont(font);
            if (families.length === 0) {
                toast.show('error', `Couldn't import ${font.family}`);
                return;
            }
            toast.show('success', `Added ${families[0]}`);
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

        <div class="scroll" bind:this={scrollEl} onscroll={measure} style="--tile-h: {TILE_H}px">
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

            <h3>
                Google Fonts
                {#if googleFiltered.length}
                    <span class="count">{googleFiltered.length}</span>
                {/if}
            </h3>
            {#if !catalog.length}
                <p class="muted">Loading catalog…</p>
            {:else if !googleFiltered.length}
                <p class="muted">No matches.</p>
            {:else}
                <!-- Virtualized grid: the outer div reserves the full scroll
                     height; only the windowed rows are rendered, offset into
                     place by `windowTop`. Column count is measured, so it matches
                     the CSS `repeat(columns, …)` the row math assumes. -->
                <div class="virtual" bind:this={virtualEl} style="height: {win.gridHeight}px">
                    <div
                        class="grid window"
                        style="top: {win.windowTop}px; grid-template-columns: repeat({win.columns}, minmax(0, 1fr))"
                    >
                        {#each googleWindow as font (font.family)}
                            <button
                                class="tile"
                                style="font-family: '{font.family}', sans-serif"
                                onclick={() => pickGoogle(font)}
                                disabled={importing.has(font.family)}
                                title={font.family}
                            >
                                <span class="sample">{sampleFor(font.family)}</span>
                                <span class="name">
                                    {#if importing.has(font.family)}
                                        <span class="importing">Importing…</span>
                                    {:else}
                                        {font.family}
                                    {/if}
                                </span>
                            </button>
                        {/each}
                    </div>
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
        /* Offset parent for the virtualized grid's absolute window. */
        position: relative;
    }

    h3 {
        font-size: 12px;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--text-muted);
        margin: 12px 0 6px;
    }
    .count {
        text-transform: none;
        letter-spacing: 0;
        opacity: 0.7;
        margin-left: 6px;
    }

    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
        gap: 8px;
    }

    /* Reserves the full scroll height of the (virtualized) Google grid so the
       scrollbar reflects the whole catalog; the rendered window is absolutely
       positioned within it. */
    .virtual {
        position: relative;
        width: 100%;
    }
    .grid.window {
        position: absolute;
        left: 0;
        right: 0;
        /* Column template is set inline from the measured column count. */
    }

    .tile {
        /* Fixed height so the virtual-scroll row math (`--tile-h` in JS) holds. */
        height: var(--tile-h, 74px);
        box-sizing: border-box;
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
    .importing {
        color: var(--accent);
        font-style: italic;
    }
    .muted {
        color: var(--text-muted);
        font-size: 13px;
    }
</style>
