<script lang="ts">
    import { hardenExternalAnchors } from '../links';

    // Content and readiness both arrive as props rather than off their
    // singletons, so each root states its own idea of "loaded" (the editor
    // waits for its engine; the GPU error page is already as loaded as it
    // gets) and the tests can drive both. Vitest inherits vite.config.ts's
    // `define`, so a developer with DARKLY_BANNER exported in their shell
    // would otherwise see it leak into the test process.
    let {
        html,
        ready,
        ondismiss,
    }: { html: string; ready: boolean; ondismiss: () => void } = $props();

    /** Beat between the app settling and the banner arriving, so it reads as
     *  its own event rather than as the last thing to finish loading. */
    const REVEAL_DELAY_MS = 600;

    let body = $state<HTMLElement | null>(null);
    let shown = $state(false);

    $effect(() => {
        // Re-read `html` so the effect re-runs when the markup changes.
        void html;
        if (body) hardenExternalAnchors(body);
    });

    $effect(() => {
        // The banner opens from zero height, so arriving mid-boot would reflow
        // the canvas while the engine is still setting itself up.
        if (!ready) return;
        const timer = setTimeout(() => (shown = true), REVEAL_DELAY_MS);
        return () => clearTimeout(timer);
    });

    // `suppressButtonKeyboardFocus` stamps tabindex="-1" on every button that
    // does not declare one, so Space and Tab always reach the canvas hotkeys.
    // This button declares tabindex="0" and so keeps its focus: dismissing is
    // the banner's only affordance, with no menu entry or shortcut behind it,
    // so a keyboard-only reader would otherwise be stuck with the message for
    // good. The conflict that motivates the suppression is handled here
    // instead: `hotkeys.svelte.ts` binds tinykeys on `window` and bails only
    // for an open dialog or an editable target, so activation keys stop at
    // the button rather than also firing a canvas shortcut.
    function onkeydown(e: KeyboardEvent) {
        if (e.key === 'Enter' || e.key === ' ') e.stopPropagation();
    }
</script>

<aside class="announcement" class:shown aria-label="Announcement">
    <div class="inner">
        <!-- The markup is a compile-time constant, supplied by whoever ran the
             build via DARKLY_BANNER. It is not user input and there is no path
             for a reader to influence it, so this is not an XSS surface. -->
        <div class="announcement-body" bind:this={body}>{@html html}</div>
        <button
            type="button"
            class="icon-btn close"
            tabindex="0"
            aria-label="Dismiss announcement"
            onclick={ondismiss}
            {onkeydown}
        >×</button>
    </div>
</aside>

<style>
    /* Opens from nothing on a `0fr`/`1fr` grid row, which animates to the
     * content's own height without anyone having to know what that is. */
    .announcement {
        display: grid;
        grid-template-rows: 0fr;
        flex-shrink: 0;
        background: var(--brand);
        color: var(--on-brand);
        transition: grid-template-rows 340ms cubic-bezier(0.22, 1, 0.36, 1);
    }

    .announcement.shown {
        grid-template-rows: 1fr;
    }

    .inner {
        display: flex;
        align-items: center;
        gap: 8px;
        min-height: 0;
        overflow: hidden;
        padding: 0 6px 0 12px;
        opacity: 0;
        transform: translateY(-3px);
        transition: opacity 240ms ease 120ms, transform 240ms ease 120ms;
    }

    .announcement.shown .inner {
        opacity: 1;
        transform: none;
        /* Padding rides the row rather than the grid parent, so the collapsed
         * state really is zero pixels tall. The bar is chrome above the menu
         * bar, so it stays thinner than the 32px one below it. */
        padding-block: 3px;
    }

    .announcement-body {
        flex: 1;
        min-width: 0;
        /* The shell is `overflow: hidden` with no scroll escape and the app
         * has no responsive breakpoints, so a long message on a narrow window
         * would eat the canvas with no recovery but dismissal. Cap it and let
         * the text scroll inside itself instead. A bare URL is unbreakable,
         * hence `anywhere`. */
        max-height: 20vh;
        overflow: auto;
        overflow-wrap: anywhere;
        font-size: 12px;
        line-height: 1.5;
        user-select: text;
    }

    /* `{@html}` content is not scoped to this component's styles. The build
     * author writes a plain `<a href>`; everything that makes it behave like
     * a link in this app is applied here and in `hardenExternalAnchors`. */
    .announcement-body :global(a) {
        color: var(--on-brand);
        text-decoration: underline;
        text-underline-offset: 2px;
        cursor: pointer;
    }

    .announcement-body :global(a:hover) {
        text-decoration-thickness: 2px;
    }

    /* Not `.icon-btn.square`: its fixed 32px box would set the bar's height
     * on its own. `.icon-btn` also dresses itself in the greyscale chrome
     * colours, which disappear against the brand fill. */
    .close {
        flex-shrink: 0;
        width: 18px;
        height: 18px;
        color: var(--on-brand);
        font-size: 15px;
        line-height: 1;
    }

    .close:hover {
        background: rgba(255, 255, 255, 0.18);
        color: var(--on-brand);
    }
</style>
