<script lang="ts">
    /**
     * The chain button: "these two controls are tied together". A closed chain
     * when the link is engaged, a broken one when it is not, following GIMP's
     * `GimpChainButton`, the widget between linked width/height fields.
     *
     * `bracket` draws short connector lines out of both sides into the
     * surrounding gaps, so the button visibly joins the control on its left to
     * the one on its right rather than floating between them. Use it when the
     * two linked controls sit side by side on one row; leave it off when the
     * button trails a pair that already reads as a group.
     */
    import Icon from '../icons/Icon.svelte';

    let {
        linked,
        onchange,
        label,
        bracket = false,
    }: {
        linked: boolean;
        onchange: (linked: boolean) => void;
        /** What the link does, e.g. "aspect ratio". Voiced as "Lock …" /
         *  "Unlock …" so the control announces the effect of pressing it. */
        label: string;
        bracket?: boolean;
    } = $props();

    let title = $derived(`${linked ? 'Unlock' : 'Lock'} ${label}`);
</script>

<button
    type="button"
    class="link-toggle"
    class:active={linked}
    class:bracket
    aria-pressed={linked}
    aria-label={title}
    {title}
    onclick={() => onchange(!linked)}
>
    <Icon name={linked ? 'fa6-solid:link' : 'fa6-solid:link-slash'} />
</button>

<style>
    .link-toggle {
        display: flex;
        align-items: center;
        justify-content: center;
        flex: none;
        width: 34px;
        height: 34px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: var(--radius-md);
        color: var(--text-muted);
        font-size: 14px;
        cursor: pointer;
    }
    .link-toggle:hover {
        background: var(--bg-hover);
        color: var(--text);
    }
    .link-toggle.active {
        background: var(--accent);
        border-color: var(--accent);
        color: #fff;
    }

    /* Connector stubs reaching into the row's gaps on both sides. They are
       the whole point of the bracket form: the eye reads one joined unit,
       and the accent color makes an engaged link obvious at a glance. */
    .bracket::before,
    .bracket::after {
        content: '';
        position: absolute;
        top: 50%;
        width: 7px;
        height: 2px;
        background: var(--bg-hover);
        transform: translateY(-50%);
    }
    .bracket {
        position: relative;
    }
    .bracket::before {
        right: 100%;
    }
    .bracket::after {
        left: 100%;
    }
    .bracket.active::before,
    .bracket.active::after {
        background: var(--accent);
    }
</style>
