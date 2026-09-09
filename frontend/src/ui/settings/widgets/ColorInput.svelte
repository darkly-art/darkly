<script lang="ts">
    /**
     * A hex-valued color control: a swatch that opens the color wheel, and a
     * hex field. `oninput` reports every wheel move, `onchange` a release or a
     * committed hex entry, the `live` / `commit` split the other settings
     * widgets use. A host with no live path wires only `onchange`.
     */
    import { colorToHexRgb, hexToColor } from '../../../lib/color';
    import ColorPopup from '../../color/ColorPopup.svelte';
    import { newId } from '../../../lib/id';

    // `value` is untyped where it comes from: a settings row hands over
    // whatever `config.get` returned, which is `undefined` for an unset pref.
    // Both that and a malformed string read as black rather than throwing.
    type Props = { value: string | undefined; oninput?: (v: string) => void; onchange: (v: string) => void };
    let { value, oninput, onchange }: Props = $props();

    // Each control is its own dismiss scope so two on one page never hold
    // each other open.
    const scope = newId('color-input');
    let open = $state(false);
    let swatch = $state<HTMLButtonElement>();

    let color = $derived(hexToColor(value ?? '') ?? { r: 0, g: 0, b: 0, a: 255 });

    // A malformed entry is ignored and the field snaps back on the next
    // value change, rather than silently painting black.
    function onHexChange(e: Event & { currentTarget: HTMLInputElement }) {
        const c = hexToColor(e.currentTarget.value);
        if (!c) return;
        onchange(colorToHexRgb(c));
    }
</script>

<div class="row">
    <button
        class="swatch"
        bind:this={swatch}
        data-keep-open={scope}
        style:background="rgb({color.r}, {color.g}, {color.b})"
        onclick={() => (open = !open)}
        title="Pick color"
        aria-label="Pick color"
    ></button>
    <input type="text" class="hex" value={value ?? ''} onchange={onHexChange} spellcheck="false" />
</div>

{#if open}
    <ColorPopup
        value={color}
        oninput={(c) => oninput?.(colorToHexRgb(c))}
        onchange={(c) => onchange(colorToHexRgb(c))}
        onclose={() => (open = false)}
        {scope}
        anchor={swatch!}
    />
{/if}

<style>
    .row { display: inline-flex; align-items: center; gap: 8px; }
    .swatch {
        width: 32px;
        height: 28px;
        padding: 0;
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        cursor: pointer;
    }
    .hex {
        width: 110px;
        font-family: var(--font-mono, monospace);
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 8px;
        font-size: 12px;
    }
    .hex:focus { outline: 2px solid var(--accent); outline-offset: 0; border-color: transparent; }
</style>
