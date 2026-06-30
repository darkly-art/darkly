<script lang="ts">
    import { onMount } from 'svelte';
    import { app } from '../../state/app.svelte';
    import { textSession } from '../../tools/text.svelte';
    import {
        createTextFromPending,
        queueTextContent,
        flushTextContent,
        applyStyleDefaults,
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

    // The active layer, or null in pending mode (a placement with no layer yet).
    let { node }: { node: { id: number; type: string } | null } = $props();

    const ALIGN_OPTIONS: [string, string][] = [
        ['start', 'Left'],
        ['center', 'Center'],
        ['end', 'Right'],
        ['justified', 'Justify'],
    ];

    let fontOptions = $state<[string, string][]>([['Noto Sans', 'Noto Sans']]);

    onMount(async () => {
        if (!app.engine) return;
        const res = (await app.engine.send('list_fonts')) as { fonts: string[] } | null;
        if (res?.fonts?.length) fontOptions = res.fonts.map((f) => [f, f] as [string, string]);
    });

    /** One editor block — a text object, or the pending placeholder. The `key`
     *  is the load-bearing detail: both modes render through the same keyed
     *  `{#each}`, and the placeholder hands its key to the created object, so
     *  the textarea element survives pending→bound (caret + focus preserved). */
    interface Block {
        key: string;
        objectId: number | null;
        layer: number | null;
        content: string;
        font_family: string;
        size: number;
        weight: number;
        italic: boolean;
        align: string;
        color: Rgba;
    }

    let blocks = $state<Block[]>([]);

    // Non-reactive bookkeeping. `els`/`lastSent` track the uncontrolled
    // textareas; `localIdByObject` + `pendingLocalId` mint the stable keys.
    const els = new Map<string, HTMLTextAreaElement>();
    const lastSent = new Map<string, string>();
    const localIdByObject = new Map<number, number>();
    let pendingLocalId: number | null = null;
    let nextLocalId = 0;
    let creating = false;

    function foregroundTuple(): Rgba {
        const c = app.foreground;
        return [c.r, c.g, c.b, c.a];
    }

    function currentStyle(): FullStyle {
        return {
            font_family: textSession.fontFamily,
            size: textSession.size,
            weight: textSession.weight,
            italic: textSession.italic,
            align: textSession.align,
        };
    }

    function pendingBlock(local: number): Block {
        return {
            key: `b${local}`,
            objectId: null,
            layer: null,
            content: '',
            ...currentStyle(),
            color: foregroundTuple(),
        };
    }

    function setBoundBlocks(layerId: number, objs: any[]) {
        blocks = objs.map((o) => {
            let local = localIdByObject.get(o.object);
            if (local === undefined) {
                // The first newly-seen object after a placement inherits the
                // pending key, so its textarea isn't remounted on create.
                if (pendingLocalId !== null) {
                    local = pendingLocalId;
                    pendingLocalId = null;
                } else {
                    local = nextLocalId++;
                }
                localIdByObject.set(o.object, local);
            }
            return {
                key: `b${local}`,
                objectId: o.object,
                layer: layerId,
                content: o.content,
                font_family: o.font_family,
                size: o.size,
                weight: o.weight,
                italic: o.italic,
                align: o.align,
                color: o.color as Rgba,
            };
        });
    }

    // Build the block list. Bound mode (a vector layer is active) refetches
    // `text_objects` keyed on the layer id AND `app.layerTree` (which changes
    // every refresh) so undo/redo reflects. Pending mode renders one
    // placeholder block.
    $effect(() => {
        const n = node;
        void app.layerTree; // dependency: refetch on undo/redo
        const placement = textSession.placement;
        if (n && n.type === 'vector' && app.engine) {
            const layerId = n.id;
            let cancelled = false;
            app.engine
                .send<{ objects: any[] }>('text_objects', { id: layerId })
                .then((res) => {
                    if (!cancelled) setBoundBlocks(layerId, res?.objects ?? []);
                });
            return () => {
                cancelled = true;
            };
        }
        if (placement) {
            if (pendingLocalId === null) pendingLocalId = nextLocalId++;
            // While a create is in flight its `refreshLayerTree` re-runs this
            // effect; leave the existing pending block (and its uncontrolled
            // textarea) alone so the just-typed character isn't wiped.
            if (!creating) blocks = [pendingBlock(pendingLocalId)];
        } else {
            blocks = [];
        }
    });

    // Seed the uncontrolled textareas: only on an *external* change (mount,
    // undo/redo) where the engine content differs from what we last sent. A
    // self-echo leaves the field — and the caret — untouched.
    $effect(() => {
        for (const b of blocks) {
            const el = els.get(b.key);
            if (!el) continue;
            if (shouldReseed(b.content, lastSent.get(b.key))) {
                el.value = b.content;
                lastSent.set(b.key, b.content);
            }
        }
    });

    // Focus the requested object's editor once it has rendered (avoids racing
    // the async `text_objects` fetch). Caret to end — not a mid-type event.
    $effect(() => {
        const target = textSession.focusObject;
        if (target === null) return;
        const b = blocks.find((x) => x.objectId === target);
        if (!b) return;
        const el = els.get(b.key);
        if (!el) return;
        el.focus();
        const len = el.value.length;
        el.setSelectionRange(len, len);
        textSession.focusObject = null;
    });

    function bindTextarea(el: HTMLTextAreaElement, key: string) {
        els.set(key, el);
        const b = blocks.find((x) => x.key === key);
        if (b) {
            el.value = b.content;
            lastSent.set(key, b.content);
        }
        return {
            destroy() {
                els.delete(key);
            },
        };
    }

    async function createFromPending(block: Block, el: HTMLTextAreaElement) {
        if (creating) return;
        const placement = textSession.placement;
        if (!placement) return;
        creating = true;
        const content = el.value;
        try {
            const r = await createTextFromPending(
                app,
                placement,
                content,
                currentStyle(),
                block.color,
                () => el.value,
            );
            if (!r) return;
            lastSent.set(block.key, r.latest);
            textSession.focusObject = r.objectId;
            // Show the new object's box gizmo on the canvas (the text tool's
            // onFrame attaches to whatever `editing` points at).
            textSession.editing = { layerId: r.layerId, objectId: r.objectId };
            // `placement` is cleared reactively once the new vector layer is the
            // active node (see the $effect below) — never here, so the panel can
            // never fall into the "no layer yet AND no placement" gap that would
            // unmount this whole editor mid-create.
        } finally {
            creating = false;
        }
    }

    // The pending placement is realized the moment its vector layer becomes
    // active. Clearing it here (rather than in `createFromPending`) keeps the
    // PropertiesPanel gate — `vector layer || placement` — true across the whole
    // transition: `placement` only drops once `vector` already holds.
    $effect(() => {
        if (node?.type === 'vector' && textSession.placement) {
            textSession.placement = null;
        }
    });

    function onContentInput(block: Block, e: Event) {
        const el = e.currentTarget as HTMLTextAreaElement;
        if (block.objectId === null) {
            // Pending placement: the first non-empty character creates the layer.
            lastSent.set(block.key, el.value);
            if (creating || el.value.trim().length === 0) return;
            void createFromPending(block, el);
            return;
        }
        lastSent.set(block.key, el.value);
        if (block.layer !== null) queueTextContent(app, block.layer, block.objectId, el.value);
    }

    function onStyle(block: Block, fields: StyleFields) {
        const defaults = textSession as unknown as Record<string, unknown>;
        if (block.objectId !== null && block.layer !== null) {
            dispatchStyle(app, block.layer, block.objectId, fields, defaults);
        } else {
            // Pending placement: no engine object yet — bake into the defaults.
            applyStyleDefaults(defaults, fields);
        }
        // Reflect on the local block so its controls update immediately.
        if (fields.font_family !== undefined) block.font_family = fields.font_family;
        if (fields.size !== undefined) block.size = fields.size;
        if (fields.italic !== undefined) block.italic = fields.italic;
        if (fields.align !== undefined) block.align = fields.align;
        if (fields.color !== undefined) block.color = fields.color;
    }
</script>

<div class="text-props">
    {#each blocks as block (block.key)}
        <div class="block">
            <textarea
                class="content"
                rows="2"
                spellcheck="false"
                placeholder="Type text…"
                use:bindTextarea={block.key}
                oninput={(e) => onContentInput(block, e)}
                onblur={() => flushTextContent()}
            ></textarea>

            <label class="row" title="Font family">
                <span class="label">Font</span>
                <EnumDropdown
                    value={block.font_family}
                    options={fontOptions}
                    onchange={(v) => onStyle(block, { font_family: v })}
                />
            </label>

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
                    onChange={(v) => onStyle(block, { size: Math.round(v) })}
                    title="Font size in canvas pixels."
                />
            </div>

            <label class="row" title="Horizontal alignment">
                <span class="label">Align</span>
                <EnumDropdown
                    value={block.align}
                    options={ALIGN_OPTIONS}
                    onchange={(v) => onStyle(block, { align: v })}
                />
            </label>

            <label class="row" title="Italic">
                <span class="label">Italic</span>
                <input
                    type="checkbox"
                    checked={block.italic}
                    onchange={(e) => onStyle(block, { italic: (e.currentTarget as HTMLInputElement).checked })}
                />
            </label>

            <label class="row" title="Text color">
                <span class="label">Color</span>
                <ColorInput
                    value={rgbaToHex(block.color)}
                    onchange={(hex) => {
                        const rgb = hexToRgb(hex);
                        if (rgb) onStyle(block, { color: [rgb[0], rgb[1], rgb[2], block.color[3]] });
                    }}
                />
            </label>
        </div>
    {/each}
</div>

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
</style>
