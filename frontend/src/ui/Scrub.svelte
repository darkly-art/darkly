<script lang="ts">
    import { exposedDragSpeed } from '../state/brush_graph.svelte';
    import Icon from '../icons/Icon.svelte';

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

    const DEFAULT_ICON = 'fa6-solid:sliders';
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
    <Icon name={props.icon ?? DEFAULT_ICON} class="scrub-icon" />
    <div class="bar-control-text">
        <span class="bar-control-label">{props.label}</span>
        <span class="bar-control-value">{valueText}</span>
    </div>
{/snippet}

{#if props.mode === 'drag'}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="scrub bar-control"
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
        class="scrub bar-control toggle"
        class:on={props.active}
        title={props.title}
        onclick={props.onToggle}
    >
        {@render body()}
    </button>
{/if}

<style>
    /* Base look comes from the shared `.bar-control`; the rules here are
       scrub-specific behavior: drag cursor, the accent active/dragging
       state, tabular value digits, and the button reset. */
    .scrub {
        flex-shrink: 0;
        cursor: col-resize;
    }

    .scrub.dragging,
    .scrub.on {
        background: var(--accent);
    }

    .scrub.dragging :global(.scrub-icon),
    .scrub.dragging .bar-control-label,
    .scrub.dragging .bar-control-value,
    .scrub.on :global(.scrub-icon),
    .scrub.on .bar-control-label,
    .scrub.on .bar-control-value {
        color: #ffffff;
    }

    .toggle {
        border: none;
        font: inherit;
        cursor: pointer;
    }

    .scrub :global(.scrub-icon) {
        font-size: 14px;
        color: var(--text-muted);
    }

    /* Values are numeric — keep digits from shifting width while scrubbing. */
    .scrub .bar-control-value {
        font-variant-numeric: tabular-nums;
    }
</style>
