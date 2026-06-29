import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';
import { canvasToScreen } from '../canvas/coordinates';
import TextOptions from '../ui/TextOptions.svelte';
import type { Tool, ToolContext } from './registry';
import { buildCommit, type CommitRequest, type EditState, type Rgba } from './text_commit';

/** What `text_object_info` reports for re-opening an existing text object. */
interface TextObjectInfo {
    content: string;
    font_family: string;
    size: number;
    weight: number;
    italic: boolean;
    align: string;
    color: [number, number, number, number];
    ox: number;
    oy: number;
    width: number;
    height: number;
}

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
    /** The object currently open for editing, or null when placing new. When
     *  set, TextOptions style changes also dispatch a live `set_text_style`. */
    activeEdit = $state<{ layerId: number; objectId: number } | null>(null);
}

export const textSession = new TextSession();

/** Push a live style change to the object currently being edited, if any.
 *  When `activeEdit` is null (placing a new block) this is a no-op — the style
 *  is baked into `add_text` at commit instead. */
export async function pushStyleEdit(fields: {
    font_family?: string;
    size?: number;
    weight?: number;
    italic?: boolean;
    align?: string;
}): Promise<void> {
    const edit = textSession.activeEdit;
    if (!edit || !app.engine) return;
    await app.engine.send('set_text_style', { id: edit.layerId, object: edit.objectId, ...fields });
    restyleActiveOverlay();
    app.requestFrame();
}

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

function beginEdit(
    ctx: ToolContext,
    state: EditState,
    screenX: number,
    screenY: number,
    initial: string,
    color: Rgba,
) {
    cancelEdit(); // tear down any prior overlay without committing

    const container = ctx.canvasEl.parentElement;
    if (!container) return;

    const el = document.createElement('div');
    el.contentEditable = 'true';
    el.spellcheck = false;
    el.textContent = initial;
    el.setAttribute('data-darkly-text-overlay', '');

    Object.assign(el.style, {
        position: 'absolute',
        left: `${screenX}px`,
        top: `${screenY}px`,
        margin: '0',
        padding: '0',
        border: 'none',
        outline: '1px dashed var(--accent, #6cf)',
        background: 'transparent',
        color: colorCss(color),
        lineHeight: '1.2',
        whiteSpace: 'pre',
        transformOrigin: 'top left',
        zIndex: '50',
        minWidth: '1ch',
        caretColor: colorCss(color),
    } as Partial<CSSStyleDeclaration>);
    applyOverlayFont(el);

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
    textSession.activeEdit =
        state.layerId !== null && state.objectId !== null
            ? { layerId: state.layerId, objectId: state.objectId }
            : null;

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
    textSession.activeEdit = null;
    return state;
}

/** Apply the current session font (family/weight/italic/size·zoom) + alignment
 *  to an overlay element. Shared by initial styling and live restyle. */
function applyOverlayFont(el: HTMLElement): void {
    const px = textSession.size * app.zoom;
    el.style.font = `${textSession.italic ? 'italic ' : ''}${textSession.weight} ${px}px "${textSession.fontFamily}", sans-serif`;
    el.style.textAlign = textSession.align === 'justified' ? 'justify' : textSession.align;
}

/** Restyle the live overlay (if any) so a panel style change is reflected in
 *  the contenteditable, not just the re-rasterized object underneath. */
function restyleActiveOverlay(): void {
    if (active) applyOverlayFont(active.el);
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

    async onPointerDown(ctx, e, cx, cy) {
        // A click while editing commits the current block first.
        if (active) {
            commitEdit();
            return;
        }
        const engine = app.engine;
        const layerId = app.activeLayerId ?? null;

        // If the active layer is a vector layer, a click on an existing text
        // object re-opens it for editing rather than placing a new block.
        if (engine && layerId !== null) {
            const hit = await engine.send<{ object: number }>('hit_test_vector_object', {
                id: layerId,
                x: cx,
                y: cy,
            });
            if (hit && hit.object >= 0) {
                const info = await engine.send<TextObjectInfo | null>('text_object_info', {
                    id: layerId,
                    object: hit.object,
                });
                if (info) {
                    // Adopt the object's style into the panel — but NOT its color
                    // (foreground is never mutated; the overlay renders in the
                    // object's own color).
                    textSession.size = info.size;
                    textSession.fontFamily = info.font_family;
                    textSession.align = info.align;
                    textSession.italic = info.italic;
                    textSession.weight = info.weight;

                    const screen = canvasToScreen(info.ox, info.oy, ctx.canvasEl);
                    const state: EditState = {
                        layerId,
                        objectId: hit.object,
                        cx: info.ox,
                        cy: info.oy,
                        anchorLayerId: layerId,
                    };
                    const [r, g, b, a] = info.color;
                    beginEdit(ctx, state, screen.x, screen.y, info.content, { r, g, b, a });
                    return;
                }
            }
        }

        // Miss (or no vector layer) → place a brand-new text block at the click.
        const state: EditState = {
            layerId: null,
            objectId: null,
            cx,
            cy,
            anchorLayerId: layerId,
        };
        beginEdit(ctx, state, e.offsetX, e.offsetY, '', app.foreground);
    },

    onPointerMove() {},
    onPointerUp() {},

    dismissOverlay() {
        cancelEdit();
    },
};
