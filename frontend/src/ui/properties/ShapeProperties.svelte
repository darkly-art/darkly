<script lang="ts">
    import { app } from '../../state/app.svelte';
    import { catalogs } from '../../state/catalogs.svelte';
    import { shapeSession } from '../../tools/shape.svelte';
    import { rgbaToHex, hexToRgb } from '../../tools/text_editor';
    import type { ShapeObjectInfo, StrokeWire } from '../../engine/protocol_gen';
    import { visibleParams, type ParamValue } from '../params/paramSchema';
    import ParamRow from '../params/ParamRow.svelte';
    import ColorInput from '../settings/widgets/ColorInput.svelte';
    import Slider from '../settings/widgets/Slider.svelte';

    // The active vector layer. Renders nothing when it holds no shape, so it
    // sits beside `TextProperties` for every vector layer.
    let { node }: { node: { id: number; type: string } } = $props();

    // The shape tool places one shape per layer, so the panel binds to the
    // topmost one. Refetched on layer-tree changes (undo/redo).
    let shape = $state<ShapeObjectInfo | null>(null);

    $effect(() => {
        const id = node.id;
        void app.layerTree;
        if (!app.engine) {
            shape = null;
            return;
        }
        let cancelled = false;
        void app.engine.api.shapeObjects({ id }).then((objs) => {
            if (!cancelled) shape = objs.at(-1) ?? null;
        });
        return () => {
            cancelled = true;
        };
    });

    const label = $derived(shape ? catalogs.displayName('shapes', shape.family) : '');

    // Every param is pushed, hidden ones included, so a style switched away
    // from and back keeps its values.
    function pushParams() {
        if (!shape || !app.engine) return;
        const params: Record<string, ParamValue> = {};
        for (const p of shape.params) params[p.name] = p.value ?? p.default;
        app.engine.api.setShapeParams({ id: node.id, object: shape.object, params });
        shapeSession.params[shape.family] = params;
        app.requestFrame();
    }

    // A zero width is no border: the engine stores none and reports `null`.
    function pushStyle(fill: ShapeObjectInfo['fill'], border: StrokeWire) {
        if (!shape || !app.engine) return;
        shape.fill = fill;
        shape.stroke = border.width > 0 ? border : null;
        app.engine.api.setShapeStyle({ id: node.id, object: shape.object, fill, stroke: border });
        shapeSession.rememberStroke(border);
        app.requestFrame();
    }

    /** The border as the panel edits it: the shape's own, or a zero-width one
     *  in the remembered color when it has none. */
    const border = $derived<StrokeWire>(
        shape?.stroke ?? { width: 0, color: [...shapeSession.borderColor] },
    );

    function hexColor(hex: string, alpha: number): [number, number, number, number] | null {
        const rgb = hexToRgb(hex);
        return rgb ? [rgb[0], rgb[1], rgb[2], alpha] : null;
    }
</script>

{#if shape}
    {@const s = shape}
    <div class="shape-props">
        <div class="header">{label}</div>

        {#each visibleParams(s.params) as param (param.name)}
            <ParamRow {param} oninput={pushParams} onchange={pushParams} />
        {/each}

        {#if s.fill}
            {@const fill = s.fill}
            <label class="row" title="Fill color">
                <span class="label">Fill</span>
                <ColorInput
                    value={rgbaToHex(fill)}
                    onchange={(hex) => {
                        const c = hexColor(hex, fill[3]);
                        if (c) pushStyle(c, border);
                    }}
                />
            </label>
        {/if}

        <div class="row" title="Border thickness in pixels; 0 draws no border. It scales with the shape when transformed.">
            <span class="label">Border</span>
            <Slider
                value={border.width}
                min={0}
                max={64}
                step={0.5}
                onchange={(width) => pushStyle(s.fill, { ...border, width })}
            />
        </div>
        <label class="row" title="Border color">
            <span class="label">Color</span>
            <ColorInput
                value={rgbaToHex(border.color)}
                onchange={(hex) => {
                    const c = hexColor(hex, border.color[3]);
                    if (!c) return;
                    // With no border there is nothing to restyle on the
                    // shape; the color waits in the session for the width.
                    if (border.width > 0) pushStyle(s.fill, { ...border, color: c });
                    else shapeSession.borderColor = c;
                }}
            />
        </label>
    </div>
{/if}

<style>
    .shape-props {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .header {
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text-muted);
        padding-bottom: 4px;
        border-bottom: 1px solid var(--bg-hover);
    }

    .row {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 22px;
        font-size: 12px;
        color: var(--text);
    }

    .label {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 56px;
    }
</style>
