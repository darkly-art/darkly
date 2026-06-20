<!-- Dev-only on-screen pointer inspector. Shows the raw `PointerEvent`
     fields the browser reports for every live pointer — type, pressure,
     tilt, coords — so device behaviour can be read off directly instead of
     assumed. Built to answer "what does iPad Safari actually report for a
     finger vs. an Apple Pencil?" (see lib/pressure.ts), but it surfaces any
     pointer on any device.

     Enabled when running the dev server, or on any build by appending
     `?pointerhud` to the URL — the latter lets it ride along to a real iPad
     over the LAN where a production bundle is served. Listeners are
     capture-phase + passive, so the HUD only observes; it never consumes or
     reorders events the canvas pipeline depends on. -->
<script lang="ts">
    const enabled =
        import.meta.env.DEV ||
        (typeof window !== 'undefined' &&
            new URLSearchParams(window.location.search).has('pointerhud'));

    type Snap = {
        id: number;
        type: string;
        pressure: number;
        tangential: number;
        tiltX: number;
        tiltY: number;
        twist: number;
        width: number;
        height: number;
        buttons: number;
        x: number;
        y: number;
        phase: 'down' | 'move' | 'up' | 'cancel';
        primary: boolean;
    };

    // Keyed by pointerId. Released pointers are retained (phase: up/cancel)
    // so a value can still be read after lifting a finger or pen.
    let pointers = $state<Record<number, Snap>>({});

    function snap(e: PointerEvent, phase: Snap['phase']): Snap {
        return {
            id: e.pointerId,
            type: e.pointerType || '(empty)',
            pressure: e.pressure,
            tangential: e.tangentialPressure,
            tiltX: e.tiltX,
            tiltY: e.tiltY,
            twist: e.twist,
            width: e.width,
            height: e.height,
            buttons: e.buttons,
            x: Math.round(e.clientX),
            y: Math.round(e.clientY),
            phase,
            primary: e.isPrimary,
        };
    }

    function record(phase: Snap['phase']) {
        return (e: PointerEvent) => {
            pointers = { ...pointers, [e.pointerId]: snap(e, phase) };
        };
    }

    // Capture-phase + passive: observe only, never consume or reorder the
    // events the canvas pipeline depends on. Bound imperatively so a build
    // without the flag attaches no global listeners at all.
    $effect(() => {
        if (!enabled) return;
        const binds: Array<[string, (e: PointerEvent) => void]> = [
            ['pointerdown', record('down')],
            ['pointermove', record('move')],
            ['pointerup', record('up')],
            ['pointercancel', record('cancel')],
        ];
        const opts = { capture: true, passive: true } as const;
        for (const [name, fn] of binds) {
            window.addEventListener(name, fn as EventListener, opts);
        }
        return () => {
            for (const [name, fn] of binds) {
                window.removeEventListener(name, fn as EventListener, opts);
            }
        };
    });

    function clear() {
        pointers = {};
    }

    const live = $derived(Object.values(pointers));
    const fmt = (n: number) => (Number.isInteger(n) ? `${n}` : n.toFixed(3));
</script>

{#if enabled}
    <div class="pointer-hud">
        <div class="hud-head">
            <span>pointer HUD</span>
            <button onclick={clear} title="Clear">clear</button>
        </div>
        {#if live.length === 0}
            <div class="hud-empty">touch / click the screen…</div>
        {:else}
            {#each live as p (p.id)}
                <div class="hud-row" class:dead={p.phase === 'up' || p.phase === 'cancel'}>
                    <div class="hud-type">
                        {p.type}{p.primary ? '' : ' (2nd)'}
                        <span class="hud-phase">{p.phase}</span>
                    </div>
                    <div class="hud-fields">
                        <span><b>pressure</b> {fmt(p.pressure)}</span>
                        <span>id {p.id}</span>
                        <span>btns {p.buttons}</span>
                        <span>tilt {fmt(p.tiltX)},{fmt(p.tiltY)}</span>
                        <span>twist {fmt(p.twist)}</span>
                        <span>tang {fmt(p.tangential)}</span>
                        <span>size {fmt(p.width)}×{fmt(p.height)}</span>
                        <span>xy {p.x},{p.y}</span>
                    </div>
                </div>
            {/each}
        {/if}
    </div>
{/if}

<style>
    .pointer-hud {
        position: fixed;
        top: 8px;
        left: 50%;
        transform: translateX(-50%);
        z-index: 99999;
        pointer-events: none;
        max-width: min(92vw, 520px);
        padding: 8px 10px;
        border-radius: 8px;
        background: rgba(0, 0, 0, 0.82);
        color: #e8e8e8;
        font: 12px/1.4 ui-monospace, "SF Mono", Menlo, Consolas, monospace;
        box-shadow: 0 2px 12px rgba(0, 0, 0, 0.4);
    }

    .hud-head {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 4px;
        opacity: 0.7;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        font-size: 10px;
    }

    .hud-head button {
        pointer-events: auto;
        background: rgba(255, 255, 255, 0.12);
        color: inherit;
        border: none;
        border-radius: 4px;
        padding: 2px 6px;
        font: inherit;
        cursor: pointer;
    }

    .hud-empty {
        opacity: 0.6;
    }

    .hud-row {
        padding: 4px 0;
        border-top: 1px solid rgba(255, 255, 255, 0.1);
    }

    .hud-row.dead {
        opacity: 0.45;
    }

    .hud-type {
        font-weight: 700;
        color: #7fd1ff;
        margin-bottom: 2px;
    }

    .hud-phase {
        font-weight: 400;
        color: #ffd47f;
        margin-left: 6px;
        text-transform: uppercase;
        font-size: 10px;
    }

    .hud-fields {
        display: flex;
        flex-wrap: wrap;
        gap: 2px 12px;
    }

    .hud-fields b {
        color: #9fffa8;
        font-weight: 700;
    }
</style>
