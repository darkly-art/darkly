<script lang="ts">
    import { app } from '../../state/app.svelte';

    interface VeilParam {
        kind: 'float' | 'int' | 'bool';
        name: string;
        min?: number;
        max?: number;
        default: number | boolean;
        value?: number | boolean;
    }

    let { veil }: {
        veil: { type: string; visible: boolean; index: number; params: VeilParam[] };
    } = $props();

    function pushParams() {
        if (!app.handle) return;
        const params: Record<string, number | boolean> = {};
        for (const p of veil.params) {
            params[p.name] = p.value ?? p.default;
        }
        app.handle.update_veil(veil.index, params);
        app.refreshVeilList();
        app.requestFrame();
    }

    function onSliderInput(param: VeilParam, e: Event) {
        const target = e.target as HTMLInputElement;
        param.value = param.kind === 'int'
            ? parseInt(target.value, 10)
            : parseFloat(target.value);
        pushParams();
    }

    function onBoolChange(param: VeilParam, e: Event) {
        param.value = (e.target as HTMLInputElement).checked;
        pushParams();
    }
</script>

{#if veil.params.length === 0}
    <div class="empty">No parameters</div>
{:else}
    {#each veil.params as param}
        <div class="row">
            <span class="label">{param.name}</span>
            {#if param.kind === 'float' || param.kind === 'int'}
                <input
                    type="range"
                    class="slider"
                    min={param.min}
                    max={param.max}
                    step={param.kind === 'int' ? 1 : ((param.max! - param.min!) / 100)}
                    value={param.value ?? param.default}
                    oninput={(e) => onSliderInput(param, e)}
                />
                <span class="value">
                    {param.kind === 'int' ? (param.value ?? param.default) : ((param.value ?? param.default) as number).toFixed(2)}
                </span>
            {:else if param.kind === 'bool'}
                <input
                    type="checkbox"
                    class="checkbox"
                    checked={(param.value ?? param.default) as boolean}
                    onchange={(e) => onBoolChange(param, e)}
                />
            {/if}
        </div>
    {/each}
{/if}

<style>
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
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .slider {
        flex: 1;
        height: 4px;
        min-width: 0;
    }

    .value {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 36px;
        text-align: right;
        font-variant-numeric: tabular-nums;
    }

    .checkbox {
        accent-color: var(--accent);
    }

    .empty {
        font-size: 12px;
        color: var(--text-dim);
        text-align: center;
        padding: 4px 0;
    }
</style>
