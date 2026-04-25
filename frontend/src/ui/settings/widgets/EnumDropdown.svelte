<script lang="ts">
    type Props = {
        value: string;
        options: [string, string][];
        onchange: (v: string) => void;
    };
    let { value, options, onchange }: Props = $props();
</script>

<select {value} onchange={(e) => onchange(e.currentTarget.value)}>
    {#each options as [k, label] (k)}
        <option value={k}>{label}</option>
    {/each}
</select>

<style>
    select {
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 8px;
        font-size: 12px;
        min-width: 180px;
    }
    select:focus { outline: 2px solid var(--accent); outline-offset: 0; border-color: transparent; }
</style>
