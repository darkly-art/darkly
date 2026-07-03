<script lang="ts">
    import { onMount } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { textSession } from '../../tools/text.svelte';
    import {
        createTextFromPending,
        queueTextContent,
        flushTextContent,
        dispatchStyle,
        shouldReseed,
        rgbaToHex,
        hexToRgb,
        type Rgba,
        type StyleFields,
        type FullStyle,
    } from '../../tools/text_editor';
    import EnumDropdown from '../settings/widgets/EnumDropdown.svelte';
    import ColorInput from '../settings/widgets/ColorInput.svelte';
    import Scrub from '../Scrub.svelte';
    import NumberSlider from '../settings/widgets/NumberSlider.svelte';
    import FontBrowser from '../fonts/FontBrowser.svelte';
    import { fontLibrary } from '../../state/font_library.svelte';
    import { fontCaps, type FontCapabilities } from '../../state/font_caps.svelte';

    // The active vector layer, or null when no vector layer is active (a
    // placement that will become a fresh layer).
    let { node }: { node: { id: number; type: string } | null } = $props();

    const ALIGN_OPTIONS: [string, string][] = [
        ['start', 'Left'],
        ['center', 'Center'],
        ['end', 'Right'],
        ['justified', 'Justify'],
    ];

    /** Friendly labels for the common registered variable-font axis tags; an
     *  unknown custom axis falls back to its raw 4-char tag. Lives beside the
     *  axis UI it drives. */
    const AXIS_LABELS: Record<string, string> = {
        wght: 'Weight',
        wdth: 'Width',
        opsz: 'Optical Size',
        slnt: 'Slant',
        GRAD: 'Grade',
        ital: 'Italic',
    };

    const axisLabel = (tag: string): string => AXIS_LABELS[tag] ?? tag;

    /** Content a new text object is seeded with — shown selected so the first
     *  keystroke replaces it. */
    const SEED_TEXT = 'text';

    // The engine's own families (the fallback + anything already registered),
    // fetched once; merged reactively with the personal library so uploaded /
    // Google-imported fonts appear in the quick list without a refetch.
    let engineFonts = $state<string[]>(['Noto Sans']);
    let fontBrowserOpen = $state(false);

    onMount(async () => {
        void fontLibrary.loadAll();
        if (!app.engine) return;
        const res = await app.engine.api.listFonts();
        if (res?.fonts?.length) engineFonts = res.fonts;
    });

    const fontOptions = $derived.by(() => {
        const set = new Set<string>([...engineFonts, ...fontLibrary.families]);
        // Include the current block's family even if it's not (yet) registered
        // in this handle, so the dropdown never shows a blank selection.
        if (block?.font_family) set.add(block.font_family);
        return [...set].sort((a, b) => a.localeCompare(b)).map((f) => [f, f] as [string, string]);
    });

    function onFontPicked(family: string): void {
        onStyle({ font_family: family });
    }

    /** The one text object the panel edits — a vector layer can own many, but
     *  only the selected one is shown (the user picks it by clicking it on the
     *  canvas; the text tool tracks it in `textSession.editing`). */
    interface Block {
        objectId: number;
        layer: number;
        content: string;
        font_family: string;
        size: number;
        variations: Record<string, number>;
        letter_spacing: number;
        word_spacing: number;
        line_height: number;
        italic: boolean;
        align: string;
        color: Rgba;
    }

    let block = $state<Block | null>(null);

    // The active family's capabilities (variable axes + real italic face), which
    // drive the font-driven controls. Fetched on family change; empty until then.
    let caps = $state<FontCapabilities>({ italic: false, axes: [] });

    $effect(() => {
        void fontLibrary.families; // re-fetch when the library changes (re-import)
        const family = block?.font_family;
        if (!family || !app.engine) {
            caps = { italic: false, axes: [] };
            return;
        }
        let cancelled = false;
        void fontCaps(app.engine, family).then((c) => {
            if (!cancelled) caps = c;
        });
        return () => {
            cancelled = true;
        };
    });

    // The textarea is uncontrolled — its value is set imperatively. `lastSent`
    // (by object id) lets us reseed only on an *external* change (undo/redo), not
    // a self-echo, so the caret survives.
    let textareaEl = $state<HTMLTextAreaElement | undefined>(undefined);
    const lastSent = new Map<number, string>();
    let creating = false;
    // The next focus should select all (a fresh create, so typing replaces the
    // seed); otherwise the caret goes to the end.
    let selectAllOnFocus = false;

    function foregroundTuple(): Rgba {
        const c = app.foreground;
        return [c.r, c.g, c.b, c.a];
    }

    function currentStyle(): FullStyle {
        return {
            font_family: textSession.fontFamily,
            size: textSession.size,
            variations: { ...textSession.variations },
            letter_spacing: textSession.letterSpacing,
            word_spacing: textSession.wordSpacing,
            line_height: textSession.lineHeight,
            italic: textSession.italic,
            align: textSession.align,
        };
    }

    function toBlock(layerId: number, o: any): Block {
        return {
            objectId: o.object,
            layer: layerId,
            content: o.content,
            font_family: o.font_family,
            size: o.size,
            variations: (o.variations ?? {}) as Record<string, number>,
            letter_spacing: o.letter_spacing ?? 0,
            word_spacing: o.word_spacing ?? 0,
            line_height: o.line_height ?? 1.2,
            italic: o.italic,
            align: o.align,
            color: o.color as Rgba,
        };
    }

    /** Which object to show: the selected (`editing`) one on this layer, else the
     *  topmost — so a freshly-selected layer still shows something to edit. */
    function selectedObject(objs: any[]): any | null {
        const e = textSession.editing;
        if (e && node && e.layerId === node.id) {
            const hit = objs.find((o) => o.object === e.objectId);
            if (hit) return hit;
        }
        return objs[objs.length - 1] ?? null;
    }

    // Resolve the single editable block for the active layer's selected object,
    // refetched on layer-tree changes (undo/redo) and selection changes.
    $effect(() => {
        const n = node;
        void app.layerTree; // refetch on undo/redo
        void textSession.editing; // re-resolve when the selected object changes
        if (!(n && n.type === 'vector' && app.engine)) {
            block = null;
            return;
        }
        const layerId = n.id;
        let cancelled = false;
        app.engine.api
            .textObjects({ id: layerId })
            .then((res) => {
                if (cancelled) return;
                const o = selectedObject(res?.objects ?? []);
                block = o ? toBlock(layerId, o) : null;
            });
        return () => {
            cancelled = true;
        };
    });

    // A placement (click/drag with the text tool) creates an object immediately,
    // seeded with "text" and selected — so the word appears on the canvas at
    // once and the first keystroke replaces it.
    $effect(() => {
        const placement = textSession.placement;
        if (!placement || creating || !app.engine) return;
        void createFromPlacement(placement);
    });

    async function createFromPlacement(placement: NonNullable<typeof textSession.placement>) {
        creating = true;
        try {
            // A vector layer already active → add the object to it (many objects
            // per layer); otherwise a new layer is born.
            const target = node?.type === 'vector' ? node.id : null;
            const r = await createTextFromPending(
                app,
                placement,
                SEED_TEXT,
                currentStyle(),
                foregroundTuple(),
                () => SEED_TEXT,
                target,
                (layerId, objectId) => {
                    // Select the new object (drives this panel and the box gizmo).
                    textSession.editing = { layerId, objectId };
                },
            );
            if (!r) return;
            lastSent.set(r.objectId, r.latest);
            selectAllOnFocus = true;
            textSession.focusObject = r.objectId;
            textSession.placement = null;
        } finally {
            creating = false;
        }
    }

    // Seed the uncontrolled textarea on an *external* change (mount, undo/redo,
    // switching to another object) where the engine content differs from what we
    // last sent. A self-echo leaves the field — and the caret — untouched.
    $effect(() => {
        if (!block || !textareaEl) return;
        if (shouldReseed(block.content, lastSent.get(block.objectId))) {
            textareaEl.value = block.content;
            lastSent.set(block.objectId, block.content);
        }
    });

    // Focus the selected object's editor once it has rendered. Select-all for a
    // fresh create (typing replaces the seed); caret to end for an existing one.
    $effect(() => {
        const target = textSession.focusObject;
        if (target === null || !block || block.objectId !== target || !textareaEl) return;
        textareaEl.focus();
        if (selectAllOnFocus) {
            textareaEl.select();
            selectAllOnFocus = false;
        } else {
            const len = textareaEl.value.length;
            textareaEl.setSelectionRange(len, len);
        }
        textSession.focusObject = null;
    });

    function bindTextarea(el: HTMLTextAreaElement) {
        textareaEl = el;
        if (block) {
            el.value = block.content;
            lastSent.set(block.objectId, block.content);
        }
        return {
            destroy() {
                if (textareaEl === el) textareaEl = undefined;
            },
        };
    }

    function onContentInput(e: Event) {
        if (!block) return;
        const el = e.currentTarget as HTMLTextAreaElement;
        lastSent.set(block.objectId, el.value);
        queueTextContent(app, block.layer, block.objectId, el.value);
    }

    function onStyle(fields: StyleFields) {
        if (!block) return;
        const defaults = textSession as unknown as Record<string, unknown>;
        dispatchStyle(app, block.layer, block.objectId, fields, defaults);
        // Reflect on the local block so its controls update immediately.
        if (fields.font_family !== undefined) block.font_family = fields.font_family;
        if (fields.size !== undefined) block.size = fields.size;
        // Variations merge (an untouched axis stays as it was), matching the
        // engine-side merge — editing one slider never resets the others.
        if (fields.variations !== undefined)
            block.variations = { ...block.variations, ...fields.variations };
        if (fields.letter_spacing !== undefined) block.letter_spacing = fields.letter_spacing;
        if (fields.word_spacing !== undefined) block.word_spacing = fields.word_spacing;
        if (fields.line_height !== undefined) block.line_height = fields.line_height;
        if (fields.italic !== undefined) block.italic = fields.italic;
        if (fields.align !== undefined) block.align = fields.align;
        if (fields.color !== undefined) block.color = fields.color;
    }
</script>

<div class="text-props">
    {#if block}
        <div class="block">
            <!-- Remount the (uncontrolled) textarea per object so switching the
                 selected object reseeds it; same-object content changes (undo)
                 are handled by the reseed $effect without a remount. -->
            {#key block.objectId}
                <textarea
                    class="content"
                    rows="2"
                    spellcheck="false"
                    placeholder="Type text…"
                    use:bindTextarea
                    oninput={onContentInput}
                    onblur={() => flushTextContent()}
                ></textarea>
            {/key}

            <label class="row" title="Font family">
                <span class="label">Font</span>
                <EnumDropdown
                    value={block.font_family}
                    options={fontOptions}
                    onchange={(v) => onStyle({ font_family: v })}
                />
            </label>

            <div class="row">
                <span class="label"></span>
                <button type="button" class="browse" onclick={() => (fontBrowserOpen = true)}>
                    Browse / upload…
                </button>
            </div>

            <div class="row">
                <span class="label">Size</span>
                <Scrub
                    mode="drag"
                    label="Size"
                    value={block.size}
                    min={4}
                    max={512}
                    default={48}
                    formatValue={(v) => String(Math.round(v))}
                    onChange={(v) => onStyle({ size: Math.round(v) })}
                    title="Font size in canvas pixels."
                />
            </div>

            <!-- One slider per variable-font axis the family exposes, with the
                 font's real range. An untouched axis stays absent from the map
                 (value falls back to the axis default). -->
            {#each caps.axes as axis (axis.tag)}
                <div class="row" title={`${axisLabel(axis.tag)} (${axis.tag} axis)`}>
                    <span class="label">{axisLabel(axis.tag)}</span>
                    <NumberSlider
                        value={block.variations[axis.tag] ?? axis.default}
                        min={axis.min}
                        max={axis.max}
                        integer={axis.tag === 'wght'}
                        onchange={(v) => onStyle({ variations: { [axis.tag]: v } })}
                    />
                </div>
            {/each}

            <label class="row" title="Horizontal alignment">
                <span class="label">Align</span>
                <EnumDropdown
                    value={block.align}
                    options={ALIGN_OPTIONS}
                    onchange={(v) => onStyle({ align: v })}
                />
            </label>

            {#if caps.italic}
                <label class="row" title="Italic">
                    <span class="label">Italic</span>
                    <input
                        type="checkbox"
                        checked={block.italic}
                        onchange={(e) =>
                            onStyle({ italic: (e.currentTarget as HTMLInputElement).checked })}
                    />
                </label>
            {/if}

            <div class="row" title="Extra space between letters, in canvas pixels.">
                <span class="label">Letter Spacing</span>
                <NumberSlider
                    value={block.letter_spacing}
                    min={-10}
                    max={50}
                    onchange={(v) => onStyle({ letter_spacing: v })}
                />
            </div>

            <div class="row" title="Extra space between words, in canvas pixels.">
                <span class="label">Word Spacing</span>
                <NumberSlider
                    value={block.word_spacing}
                    min={-10}
                    max={100}
                    onchange={(v) => onStyle({ word_spacing: v })}
                />
            </div>

            <div class="row" title="Line height, as a multiple of the font's natural height.">
                <span class="label">Line Height</span>
                <NumberSlider
                    value={block.line_height}
                    min={0.5}
                    max={3}
                    onchange={(v) => onStyle({ line_height: v })}
                />
            </div>

            <label class="row" title="Text color">
                <span class="label">Color</span>
                <ColorInput
                    value={rgbaToHex(block.color)}
                    onchange={(hex) => {
                        const rgb = hexToRgb(hex);
                        if (rgb) onStyle({ color: [rgb[0], rgb[1], rgb[2], block.color[3]] });
                    }}
                />
            </label>
        </div>
    {/if}
</div>

<FontBrowser
    bind:open={fontBrowserOpen}
    onSelect={onFontPicked}
    onClose={() => (fontBrowserOpen = false)}
/>

<style>
    .text-props {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }

    .block {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .content {
        width: 100%;
        box-sizing: border-box;
        resize: vertical;
        min-height: 44px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        color: var(--text);
        font-size: 13px;
        padding: 6px 8px;
        outline: none;
    }
    .content:focus {
        border-color: var(--accent);
    }

    .row {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 22px;
    }

    .label {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 56px;
    }

    .browse {
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        color: var(--text);
        font-size: 12px;
        padding: 4px 10px;
        cursor: pointer;
    }
    .browse:hover {
        border-color: var(--accent);
    }
</style>
