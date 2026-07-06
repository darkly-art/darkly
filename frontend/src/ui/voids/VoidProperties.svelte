<script lang="ts">
    import { app } from '../../state/app.svelte';
    import Icon from '../../icons/Icon.svelte';
    import Slider from '../settings/widgets/Slider.svelte';

    interface VoidParam {
        kind: 'float' | 'int' | 'bool' | 'string';
        name: string;
        min?: number;
        max?: number;
        default: number | boolean | string;
        value?: number | boolean | string;
    }

    let { node }: {
        node: { id: number; voidType: string; params: VoidParam[] };
    } = $props();

    function pushParams() {
        if (!app.engine) return;
        const params: Record<string, number | boolean | string> = {};
        for (const p of node.params) {
            params[p.name] = p.value ?? p.default;
        }
        app.engine.api.setVoidParams({ id: node.id, params });
        app.refreshLayerTree();
        app.requestFrame();
    }

    function onSliderChange(param: VoidParam, v: number) {
        param.value = v;
        pushParams();
    }

    function onBoolChange(param: VoidParam, e: Event) {
        param.value = (e.target as HTMLInputElement).checked;
        pushParams();
    }

    function onStringChange(param: VoidParam, e: Event) {
        param.value = (e.target as HTMLInputElement).value;
        pushParams();
    }

    function randomizeSeed() {
        const seedParam = node.params.find((p) => p.name === 'seed');
        if (!seedParam) return;
        seedParam.value = Math.floor(Math.random() * 1_000_000);
        pushParams();
    }

    const voidLabel = $derived(app.voidDisplayName(node.voidType));

    // Capture kind (camera / screenshare / Blender stream) for this void, or
    // undefined for procedural voids — the single signal that gates every
    // stream-related affordance below.
    const captureKind = $derived(app.voidCaptureKind.get(node.voidType));

    // Stream-backed voids surface source-level errors here so the user sees a
    // human-readable reason ("Camera access was denied", "Could not connect to
    // the Blender stream…", …) instead of a silently-transparent layer.
    const streamError = $derived(
        captureKind ? app.streamSourceFor(node.id)?.error ?? null : null,
    );

    // True for a stream-backed void whose layer exists but isn't currently
    // streaming — either loaded from a `.darkly` (showing the saved last frame)
    // or stopped externally (the browser's "Stop sharing" bar, a Blender
    // disconnect). The button lets the user explicitly (re)connect. The session
    // opt-in is cleared on external stop, so this re-appears then too.
    const showResume = $derived(
        !!captureKind
            && !isFrozen(node.params)
            && !app.streamSessionStarted.has(node.id),
    );

    // Per-kind verb for the (re)connect button.
    const resumeLabel = $derived(
        captureKind === 'stream'
            ? 'Connect to Blender'
            : captureKind === 'display'
              ? 'Resume screen share'
              : 'Resume camera',
    );

    function isFrozen(params: VoidParam[]): boolean {
        const f = params.find((p) => p.name === 'freeze');
        return (f?.value ?? f?.default) === true;
    }

    function resumeStream() {
        if (!captureKind) return;
        // For camera / screenshare this is a user gesture — acquire + start
        // in-gesture so getDisplayMedia's activation requirement holds. For a
        // Blender `stream` void there's no permission gate; `startStreamSource`
        // connects over localhost HTTP immediately.
        app.markStreamVoidStarted(node.id);
        app.startStreamSource(node.id, captureKind);
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
        <Icon name="fa6-solid:dice" />
    </button>
</div>

{#if streamError}
    <div class="notice">
        <Icon name="fa6-solid:triangle-exclamation" />
        <span>{streamError}</span>
    </div>
{/if}

{#if showResume}
    <button class="resume-btn" onclick={resumeStream}>
        <Icon name="fa6-solid:video" />
        <span>{resumeLabel}</span>
    </button>
{/if}

{#if node.params.length === 0}
    <div class="empty">No parameters</div>
{:else}
    {#each node.params as param}
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
            {:else if param.kind === 'string'}
                <input
                    type="text"
                    class="text-input"
                    value={(param.value ?? param.default) as string}
                    onchange={(e) => onStringChange(param, e)}
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

    .checkbox {
        accent-color: var(--accent);
    }

    .text-input {
        flex: 1;
        min-width: 0;
        padding: 3px 6px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-sm);
        color: var(--text);
        font-size: 11px;
    }
    .text-input:focus {
        outline: none;
        border-color: var(--accent);
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
