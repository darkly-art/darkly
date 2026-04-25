<script lang="ts">
    type Props = { value: boolean; onchange: (v: boolean) => void };
    let { value, onchange }: Props = $props();
</script>

<label class="toggle">
    <input type="checkbox" checked={value} onchange={(e) => onchange(e.currentTarget.checked)} />
    <span class="track"><span class="thumb"></span></span>
</label>

<style>
    .toggle {
        display: inline-flex;
        align-items: center;
        cursor: pointer;
    }
    input { position: absolute; opacity: 0; pointer-events: none; }
    .track {
        width: 36px;
        height: 20px;
        border-radius: 10px;
        background: var(--bg-hover);
        position: relative;
        transition: background 0.15s;
    }
    .thumb {
        position: absolute;
        top: 2px;
        left: 2px;
        width: 16px;
        height: 16px;
        border-radius: 50%;
        background: var(--text-muted);
        transition: transform 0.15s, background 0.15s;
    }
    input:checked ~ .track { background: var(--accent); }
    input:checked ~ .track .thumb { transform: translateX(16px); background: #fff; }
    input:focus-visible ~ .track { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
