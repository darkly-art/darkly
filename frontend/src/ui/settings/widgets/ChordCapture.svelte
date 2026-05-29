<script lang="ts">
    import { formatHotkey } from '../../../config/store.svelte';

    type Props = {
        /** Bare chord (no `site:` prefix). May be `""` for an unbound row. */
        value: string;
        /** Called with the new bare chord. Empty string == cleared. */
        onchange: (chord: string) => void;
        /** When false, mouse chords are rejected — only keyboard chords are
         *  captured. Used by the schema-pref hotkey widget (`nav.trigger`
         *  etc.) where mouse bindings are nonsensical. */
        acceptMouse?: boolean;
        /** When true, the widget enters capture mode the first time it
         *  mounts. Used by "+ Add trigger" so the new row immediately
         *  prompts for a key without a second click. */
        autostart?: boolean;
        /** Tooltip text shown when there's a conflict; renders a warning
         *  border and is read out on hover. */
        conflict?: string | null;
        /** Fallback display when value is empty. */
        placeholder?: string;
    };
    let {
        value,
        onchange,
        acceptMouse = true,
        autostart = false,
        conflict = null,
        placeholder = '(unbound)',
    }: Props = $props();

    let buttonEl: HTMLButtonElement | undefined = $state();
    let capturing = $state(false);
    let captureCleanup: (() => void) | null = null;
    let autostarted = false;

    /** Begin capture. Installs document-level keydown + pointerdown
     *  listeners in the *capture phase* so we win the race against
     *  tinykeys' window-bubble handler (and don't depend on the button
     *  retaining focus — the prior `<button onblur=stop onkeydown=…>`
     *  pattern broke whenever the browser detoured focus, e.g. inside a
     *  `<dialog>.showModal()`). */
    function beginCapture() {
        if (capturing) return;
        capturing = true;

        const stop = () => {
            capturing = false;
            captureCleanup?.();
            captureCleanup = null;
        };

        const onKey = (e: KeyboardEvent) => {
            // Swallow the event regardless of what we end up doing with
            // it — otherwise capturing a hotkey would *also* fire its
            // dispatch handler. Pure-modifier presses fall through after
            // preventDefault to wait for the real keystroke.
            e.preventDefault();
            e.stopPropagation();

            if (e.code === 'Escape') { stop(); return; }

            if (e.code === 'Backspace' || e.code === 'Delete') {
                onchange('');
                stop();
                return;
            }

            // Pure modifiers are prefixes, not keystrokes. Wait for the
            // real key.
            if ([
                'ShiftLeft', 'ShiftRight',
                'ControlLeft', 'ControlRight',
                'AltLeft', 'AltRight',
                'MetaLeft', 'MetaRight',
            ].includes(e.code)) {
                return;
            }

            const parts: string[] = [];
            if (e.ctrlKey || e.metaKey) parts.push('$mod');
            if (e.shiftKey) parts.push('Shift');
            if (e.altKey) parts.push('Alt');
            parts.push(e.code);
            onchange(parts.join('+'));
            stop();
        };

        const onPointer = (e: PointerEvent) => {
            const isOnButton = !!buttonEl
                && (e.target === buttonEl
                    || buttonEl.contains(e.target as Node));
            const hasMod = e.ctrlKey || e.metaKey || e.altKey || e.shiftKey;

            if (acceptMouse && isOnButton && hasMod) {
                // Capture a mouse chord on the button itself — the only
                // safe place to register a click without sending it
                // elsewhere in the UI.
                e.preventDefault();
                e.stopPropagation();
                const mods: string[] = [];
                if (e.ctrlKey || e.metaKey) mods.push('$mod');
                if (e.altKey) mods.push('alt');
                if (e.shiftKey) mods.push('shift');
                let interaction: string;
                if (e.button === 1) interaction = 'middleClick';
                else if (e.detail === 2) interaction = 'doubleClick';
                else interaction = 'click';
                onchange(mods.length > 0
                    ? `${mods.join('+')}+${interaction}`
                    : interaction);
                stop();
                return;
            }

            if (isOnButton) {
                // Bare click on our own button while capturing — treat as
                // a no-op (don't cancel; user might be re-focusing).
                return;
            }

            // Anywhere else cancels the capture so the user can dismiss
            // by clicking outside.
            stop();
        };

        document.addEventListener('keydown', onKey, { capture: true });
        document.addEventListener('pointerdown', onPointer, { capture: true });

        captureCleanup = () => {
            document.removeEventListener('keydown', onKey, { capture: true } as EventListenerOptions);
            document.removeEventListener('pointerdown', onPointer, { capture: true } as EventListenerOptions);
        };
    }

    // Auto-start once on mount when requested. The `autostarted` guard
    // prevents re-firing if the prop changes back-and-forth.
    $effect(() => {
        if (autostart && !autostarted) {
            autostarted = true;
            beginCapture();
        }
    });

    // Belt-and-braces: tear down listeners if the component is unmounted
    // mid-capture (e.g. user closes the Settings modal while a row is
    // capturing).
    $effect(() => () => captureCleanup?.());

    const displayed = $derived(formatHotkey(value) ?? placeholder);
    const promptText = $derived(
        acceptMouse
            ? 'Press a key, or hold a modifier and click…'
            : 'Press a key…',
    );
</script>

<button
    type="button"
    bind:this={buttonEl}
    class="capture"
    class:capturing
    class:has-conflict={!!conflict}
    onclick={beginCapture}
    title={conflict
        ?? (acceptMouse
            ? 'Click, then press a key — or hold a modifier and click here'
            : 'Click, then press a key combination')}
>
    {#if capturing}
        <span class="hint">{promptText}</span>
    {:else}
        <span class="value">{displayed}</span>
    {/if}
</button>

<style>
    .capture {
        font-family: var(--font-mono, monospace);
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        color: var(--text);
        cursor: pointer;
        padding: 5px 10px;
        font-size: 12px;
        min-width: 140px;
        text-align: left;
    }
    .capture:hover { border-color: color-mix(in srgb, var(--accent) 60%, var(--bg-hover)); }
    .capture.capturing {
        border-color: var(--accent);
        background: color-mix(in srgb, var(--accent) 10%, var(--bg-hover));
    }
    .capture.has-conflict { border-color: var(--danger, #e74c3c); }
    .hint { color: var(--text-muted); font-style: italic; }
    .value { color: var(--text); }
</style>
