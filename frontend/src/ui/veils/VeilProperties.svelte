<script lang="ts">
    import { app } from '../../state/app.svelte';
    import Slider from '../settings/widgets/Slider.svelte';

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
        if (!app.engine) return;
        const params: Record<string, number | boolean> = {};
        for (const p of veil.params) {
            params[p.name] = p.value ?? p.default;
        }
        app.engine.api.updateVeil({ index: veil.index, params });
        app.refreshVeilList();
        app.requestFrame();
    }

    function onSliderChange(param: VeilParam, v: number) {
        param.value = v;
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
                <Slider
                    value={(param.value ?? param.default) as number}
                    min={param.min ?? 0}
                    max={param.max ?? 1}
                    integer={param.kind === 'int'}
                    onchange={(v) => onSliderChange(param, v)}
                    format={(v) => (param.kind === 'int' ? String(v) : v.toFixed(2))}
                />
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
