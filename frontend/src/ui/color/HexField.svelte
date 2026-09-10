<script lang="ts">
    /**
     * A `#rrggbb` entry for a byte color. A malformed entry is ignored and the
     * field snaps back on the next value change, rather than silently painting
     * black. Alpha is carried over from the current value.
     */
    import type { Color } from '../../state/app.svelte';
    import { colorToHexRgb, hexToColor } from '../../lib/color';

    let { value, onchange }: { value: Color; onchange: (c: Color) => void } = $props();

    let text = $state('');
    $effect(() => {
        text = colorToHexRgb(value);
    });

    function commit() {
        const c = hexToColor(text);
        if (!c) return;
        onchange({ r: c.r, g: c.g, b: c.b, a: value.a });
    }
</script>

<input type="text" class="hex" bind:value={text} onchange={commit} maxlength="9" spellcheck="false" aria-label="Hex color" />

<style>
    .hex {
        width: 100%;
        box-sizing: border-box;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        padding: 4px 6px;
        border-radius: 3px;
        font-family: var(--font-mono, monospace);
        font-size: 12px;
    }
    .hex:focus {
        outline: 2px solid var(--accent);
        outline-offset: 0;
        border-color: transparent;
    }
</style>
