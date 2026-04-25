<script lang="ts">
    type Props = {
        value: number;
        min: number;
        max: number;
        step?: number;
        integer?: boolean;
        onchange: (v: number) => void;
    };
    let { value, min, max, step, integer = false, onchange }: Props = $props();
    const resolvedStep = $derived(step ?? (integer ? 1 : (max - min) / 200));

    function handle(n: number) {
        if (!Number.isFinite(n)) return;
        let v = integer ? Math.round(n) : n;
        if (v < min) v = min;
        if (v > max) v = max;
        onchange(v);
    }
</script>

<div class="row">
    <input
        type="range"
        {min}
        {max}
        step={resolvedStep}
        {value}
        oninput={(e) => handle(e.currentTarget.valueAsNumber)}
    />
    <input
        type="number"
        class="num"
        {min}
        {max}
        step={resolvedStep}
        {value}
        onchange={(e) => handle(e.currentTarget.valueAsNumber)}
    />
</div>

<style>
    .row { display: flex; align-items: center; gap: 10px; width: 100%; }
    input[type="range"] { flex: 1; min-width: 0; }
    .num {
        width: 80px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 4px 6px;
        font-size: 12px;
    }
    .num:focus { outline: 2px solid var(--accent); outline-offset: 0; border-color: transparent; }
</style>
