<script lang="ts">
    type Props = { value: string; onchange: (v: string) => void };
    let { value, onchange }: Props = $props();
</script>

<input
    type="text"
    {value}
    onchange={(e) => onchange(e.currentTarget.value)}
/>

<style>
    input {
        width: 220px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 8px;
        font-size: 12px;
    }
    input:focus { outline: 2px solid var(--accent); outline-offset: 0; border-color: transparent; }
</style>
