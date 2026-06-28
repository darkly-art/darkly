import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import TextOptions from '../ui/TextOptions.svelte';
import type { Tool, ToolContext } from './registry';
import { buildCommit, type CommitRequest, type EditState } from './text_commit';

/** Persisted-ish session options for the text tool, surfaced in TextOptions. */
class TextSession {
    /** Font size in canvas pixels. */
    size = $state(48);
    /** Family name resolved against the engine font collection. */
    fontFamily = $state('Noto Sans');
    /** start | center | end | justified */
    align = $state('start');
    italic = $state(false);
    /** CSS weight, 100–900. */
    weight = $state(400);
}

export const textSession = new TextSession();

// --- DOM overlay management ----------------------------------------------
//
// On-canvas editing uses an HTML `contenteditable` overlay (the web-native
// approach Krita and Graphite both take). The overlay renders with the
// browser's font engine while committed text is rendered by parley + Vello;
// at 1:1 zoom they line up, but metrics can disagree (advance/baseline), so
// text may shift slightly on commit. De-risking that alignment is the
// documented open spike (text-tool plan, "WYSIWYG overlay alignment").

interface ActiveEditor {
    el: HTMLDivElement;
    state: EditState;
    onBlur: () => void;
    onKeyDown: (e: KeyboardEvent) => void;
}

let active: ActiveEditor | null = null;

function colorCss(c: { r: number; g: number; b: number; a: number }): string {
    return `rgba(${c.r}, ${c.g}, ${c.b}, ${c.a / 255})`;
}

function beginEdit(ctx: ToolContext, state: EditState, screenX: number, screenY: number, initial: string) {
    cancelEdit(); // tear down any prior overlay without committing

    const container = ctx.canvasEl.parentElement;
    if (!container) return;

    const el = document.createElement('div');
    el.contentEditable = 'true';
    el.spellcheck = false;
    el.textContent = initial;
    el.setAttribute('data-darkly-text-overlay', '');

    const px = textSession.size * app.zoom;
    Object.assign(el.style, {
        position: 'absolute',
        left: `${screenX}px`,
        top: `${screenY}px`,
        margin: '0',
        padding: '0',
        border: 'none',
        outline: '1px dashed var(--accent, #6cf)',
        background: 'transparent',
        color: colorCss(app.foreground),
        font: `${textSession.italic ? 'italic ' : ''}${textSession.weight} ${px}px "${textSession.fontFamily}", sans-serif`,
        lineHeight: '1.2',
        whiteSpace: 'pre',
        textAlign: textSession.align === 'justified' ? 'justify' : textSession.align,
        transformOrigin: 'top left',
        zIndex: '50',
        minWidth: '1ch',
        caretColor: colorCss(app.foreground),
    } as Partial<CSSStyleDeclaration>);

    const onKeyDown = (e: KeyboardEvent) => {
        if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            commitEdit();
        } else if (e.key === 'Escape') {
            e.preventDefault();
            cancelEdit();
        }
        // Stop global hotkeys (tool switches, etc.) from firing while typing.
        e.stopPropagation();
    };
    const onBlur = () => commitEdit();

    el.addEventListener('keydown', onKeyDown);
    el.addEventListener('blur', onBlur);
    container.appendChild(el);
    active = { el, state, onBlur, onKeyDown };

    // Focus + place caret at the end on the next tick (after attach).
    queueMicrotask(() => {
        el.focus();
        const sel = window.getSelection();
        if (sel) {
            const range = document.createRange();
            range.selectNodeContents(el);
            range.collapse(false);
            sel.removeAllRanges();
            sel.addRange(range);
        }
    });
}

function teardown(): EditState | null {
    if (!active) return null;
    const { el, onBlur, onKeyDown, state } = active;
    el.removeEventListener('keydown', onKeyDown);
    el.removeEventListener('blur', onBlur);
    el.remove();
    active = null;
    return state;
}

function commitEdit() {
    if (!active) return;
    const content = active.el.innerText;
    const state = active.state;
    teardown();
    const req = buildCommit(
        state,
        content,
        {
            size: textSession.size,
            fontFamily: textSession.fontFamily,
            align: textSession.align,
            italic: textSession.italic,
            weight: textSession.weight,
        },
        app.foreground,
    );
    void dispatchCommit(req);
}

function cancelEdit() {
    teardown();
}

async function dispatchCommit(req: CommitRequest) {
    const engine = app.engine;
    if (!engine) return;
    if (req.kind === 'cancel') return;
    if (req.kind === 'add_text') {
        const res = (await engine.send('add_text', req.payload)) as { id: number };
        await app.refreshLayerTree();
        if (res && typeof res.id === 'number') app.selectLayer(res.id);
    } else {
        await engine.send('set_text_content', req.payload);
        await app.refreshLayerTree();
    }
    app.requestFrame();
}

export const textTool: Tool = {
    id: 'text',
    icon: 'fa6-solid:font',
    group: 'paint',
    hotkeyAction: 'textTool',
    optionsComponent: TextOptions,

    onActivate() {
        // Pick up the persisted default size if the user configured one.
        const cfgSize = config.get('tools.textSize');
        if (typeof cfgSize === 'number') textSession.size = cfgSize;
    },

    onDeactivate() {
        // Commit any in-flight edit so switching tools doesn't drop typed text.
        commitEdit();
    },

    // The tool owns the canvas while editing — claim the pointer so global
    // drag chords don't intercept the placement click.
    claimsPointer() {
        return active !== null;
    },

    onPointerDown(ctx, e, cx, cy) {
        // A click while editing commits the current block first.
        if (active) {
            commitEdit();
            return;
        }
        const state: EditState = {
            layerId: null,
            cx,
            cy,
            anchorLayerId: app.activeLayerId ?? null,
        };
        beginEdit(ctx, state, e.offsetX, e.offsetY, '');
    },

    onPointerMove() {},
    onPointerUp() {},

    dismissOverlay() {
        cancelEdit();
    },
};
