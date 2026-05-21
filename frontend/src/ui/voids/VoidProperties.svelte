<script lang="ts">
    import { app } from '../../state/app.svelte';

    interface VoidParam {
        kind: 'float' | 'int' | 'bool';
        name: string;
        min?: number;
        max?: number;
        default: number | boolean;
        value?: number | boolean;
    }

    let { node }: {
        node: { id: number; voidType: string; params: VoidParam[] };
    } = $props();

    function pushParams() {
        if (!app.handle) return;
        const params: Record<string, number | boolean> = {};
        for (const p of node.params) {
            params[p.name] = p.value ?? p.default;
        }
        app.handle.update_void_params(node.id, params);
        app.refreshLayerTree();
        app.requestFrame();
    }

    function onSliderInput(param: VoidParam, e: Event) {
        const target = e.target as HTMLInputElement;
        param.value = param.kind === 'int'
            ? parseInt(target.value, 10)
            : parseFloat(target.value);
        pushParams();
    }

    function onBoolChange(param: VoidParam, e: Event) {
        param.value = (e.target as HTMLInputElement).checked;
        pushParams();
    }

    function randomizeSeed() {
        const seedParam = node.params.find((p) => p.name === 'seed');
        if (!seedParam) return;
        seedParam.value = Math.floor(Math.random() * 1_000_000);
        pushParams();
    }

    const voidLabel = $derived(app.voidDisplayName(node.voidType));

    // Camera voids surface MediaStream-level errors here so the user sees a
    // human-readable reason ("Camera access was denied", "No camera was
    // found", …) instead of a silently-transparent layer.
    const cameraError = $derived(
        node.voidType === 'camera' ? app.cameraSourceFor(node.id)?.error ?? null : null,
    );

    // True for a camera void whose layer exists but isn't currently
    // streaming and hasn't been opted into this session — i.e. the user
    // loaded a `.darkly` and is looking at the saved last frame. Showing
    // a "Resume" button here is how they explicitly re-grant the camera.
    const showResume = $derived(
        node.voidType === 'camera'
            && !isFrozen(node.params)
            && !app.cameraSessionStarted.has(node.id),
    );

    function isFrozen(params: VoidParam[]): boolean {
        const f = params.find((p) => p.name === 'freeze');
        return (f?.value ?? f?.default) === true;
    }

    function resumeCamera() {
        app.markCameraVoidStarted(node.id);
    }
</script>

<div class="header">
    <span class="type-label">{voidLabel}</span>
    <button
        class="randomize-btn"
        onclick={randomizeSeed}
        title="Randomize seed"
        disabled={!node.params.some((p) => p.name === 'seed')}
    >
        <i class="fa-solid fa-dice"></i>
    </button>
</div>

{#if cameraError}
    <div class="notice">
        <i class="fa-solid fa-triangle-exclamation"></i>
        <span>{cameraError}</span>
    </div>
{/if}

{#if showResume}
    <button class="resume-btn" onclick={resumeCamera}>
        <i class="fa-solid fa-video"></i>
        <span>Resume camera</span>
    </button>
{/if}

{#if node.params.length === 0}
    <div class="empty">No parameters</div>
{:else}
    {#each node.params as param}
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
    .header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        padding-bottom: 4px;
        border-bottom: 1px solid var(--bg-hover);
        margin-bottom: 2px;
    }

    .type-label {
        font-size: 11px;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text-muted);
    }

    .randomize-btn {
        width: 22px;
        height: 22px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        border-radius: var(--radius-sm);
        color: var(--text-muted);
        cursor: pointer;
        font-size: 12px;
    }
    .randomize-btn:hover:not(:disabled) {
        background: var(--bg-hover);
        color: var(--accent);
    }
    .randomize-btn:disabled {
        opacity: 0.4;
        cursor: default;
    }

    .row {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 22px;
    }

    .label {
        font-size: 11px;
        color: var(--text-muted);
        min-width: 76px;
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
        min-width: 56px;
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

    .notice {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 6px 8px;
        margin: 4px 0;
        background: color-mix(in srgb, var(--accent) 12%, transparent);
        border: 1px solid color-mix(in srgb, var(--accent) 35%, transparent);
        border-radius: var(--radius-sm);
        font-size: 11px;
        color: var(--text);
    }

    .resume-btn {
        display: flex;
        align-items: center;
        gap: 6px;
        width: 100%;
        padding: 6px 8px;
        margin: 4px 0;
        background: var(--bg-hover);
        border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
        border-radius: var(--radius-sm);
        color: var(--text);
        font-size: 11px;
        cursor: pointer;
        justify-content: center;
    }
    .resume-btn:hover {
        background: var(--bg-active);
        border-color: var(--accent);
    }
</style>
