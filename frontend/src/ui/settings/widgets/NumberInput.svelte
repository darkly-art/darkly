<script lang="ts">
    type Props = {
        value: number;
        min?: number;
        max?: number;
        integer?: boolean;
        onchange: (v: number) => void;
    };
    let { value, min, max, integer = false, onchange }: Props = $props();

    function handle(n: number) {
        if (!Number.isFinite(n)) return;
        let v = integer ? Math.round(n) : n;
        if (min !== undefined && v < min) v = min;
        if (max !== undefined && v > max) v = max;
        onchange(v);
    }
</script>

<input
    type="number"
    {min}
    {max}
    step={integer ? 1 : 'any'}
    {value}
    onchange={(e) => handle(e.currentTarget.valueAsNumber)}
/>

<style>
    input {
        width: 140px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 8px;
        font-size: 12px;
    }
    input:focus { outline: 2px solid var(--accent); outline-offset: 0; border-color: transparent; }
</style>
