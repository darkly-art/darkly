<script lang="ts">
    import { exposedDragSpeed } from '../state/brush_graph.svelte';

    type DragProps = {
        mode: 'drag';
        icon?: string;
        label: string;
        value: number;
        min: number;
        max: number;
        default: number;
        formatValue?: (v: number) => string;
        onChange: (v: number) => void;
        title?: string;
    };
    type ToggleProps = {
        mode: 'toggle';
        icon?: string;
        label: string;
        valueLabel: string;
        active: boolean;
        onToggle: () => void;
        title?: string;
    };

    let props: DragProps | ToggleProps = $props();

    let dragging = $state(false);

    const DEFAULT_ICON = 'fa-solid fa-sliders';
    const DEFAULT_FORMAT = (v: number) => v.toFixed(2);

    const valueText = $derived(
        props.mode === 'drag'
            ? (props.formatValue ?? DEFAULT_FORMAT)(props.value)
            : props.valueLabel,
    );

    function startDrag(e: PointerEvent) {
        if (props.mode !== 'drag') return;
        e.preventDefault();
        const { min, max, onChange } = props;
        const startX = e.clientX;
        const startVal = props.value;
        const speed = exposedDragSpeed(min, max);
        const el = e.currentTarget as HTMLElement;
        el.setPointerCapture(e.pointerId);
        dragging = true;
        const onMove = (ev: PointerEvent) => {
            const dx = ev.clientX - startX;
            const v = Math.min(max, Math.max(min, startVal + dx * speed));
            onChange(v);
        };
        const onUp = () => {
            dragging = false;
            el.removeEventListener('pointermove', onMove);
            el.removeEventListener('pointerup', onUp);
        };
        el.addEventListener('pointermove', onMove);
        el.addEventListener('pointerup', onUp);
    }

    function resetDefault() {
        if (props.mode !== 'drag') return;
        props.onChange(props.default);
    }
</script>

{#snippet body()}
    <i class="{props.icon ?? DEFAULT_ICON} scrub-icon"></i>
    <div class="scrub-text">
        <span class="scrub-label">{props.label}</span>
        <span class="scrub-value">{valueText}</span>
    </div>
{/snippet}

{#if props.mode === 'drag'}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="scrub"
        class:dragging
        title={props.title}
        onpointerdown={startDrag}
        ondblclick={resetDefault}
    >
        {@render body()}
    </div>
{:else}
    <button
        type="button"
        class="scrub toggle"
        class:on={props.active}
        title={props.title}
        onclick={props.onToggle}
    >
        {@render body()}
    </button>
{/if}

<style>
    .scrub {
        flex-shrink: 0;
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 10px;
        border-radius: 6px;
        cursor: col-resize;
        background: var(--bg-hover);
        transition: background 0.1s;
    }

    .scrub:hover {
        background: var(--bg-active);
    }

    .scrub.dragging,
    .scrub.on {
        background: var(--accent);
    }

    .scrub.dragging .scrub-icon,
    .scrub.dragging .scrub-label,
    .scrub.dragging .scrub-value,
    .scrub.on .scrub-icon,
    .scrub.on .scrub-label,
    .scrub.on .scrub-value {
        color: #ffffff;
    }

    .toggle {
        border: none;
        font: inherit;
        cursor: pointer;
    }

    .scrub-icon {
        font-size: 14px;
        color: var(--text-muted);
    }

    .scrub-text {
        display: flex;
        flex-direction: column;
    }

    .scrub-label {
        font-size: 9px;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        line-height: 1;
    }

    .scrub-value {
        font-size: 12px;
        color: var(--text);
        font-variant-numeric: tabular-nums;
        line-height: 1.3;
    }
</style>
