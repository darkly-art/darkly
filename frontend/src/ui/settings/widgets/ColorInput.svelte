<script lang="ts">
    type Props = { value: string; onchange: (v: string) => void };
    let { value, onchange }: Props = $props();

    // Normalize to #rrggbb; some values come in with alpha or short form and
    // the native color picker will reject those. Best-effort here.
    function normalize(v: string): string {
        if (/^#[0-9a-fA-F]{6}$/.test(v)) return v;
        if (/^#[0-9a-fA-F]{3}$/.test(v)) {
            const r = v[1], g = v[2], b = v[3];
            return `#${r}${r}${g}${g}${b}${b}`;
        }
        return '#000000';
    }
</script>

<div class="row">
    <input
        type="color"
        value={normalize(value)}
        oninput={(e) => onchange(e.currentTarget.value)}
    />
    <input
        type="text"
        class="hex"
        {value}
        onchange={(e) => onchange(e.currentTarget.value)}
    />
</div>

<style>
    .row { display: inline-flex; align-items: center; gap: 8px; }
    input[type="color"] {
        width: 32px;
        height: 28px;
        padding: 0;
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        background: transparent;
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
