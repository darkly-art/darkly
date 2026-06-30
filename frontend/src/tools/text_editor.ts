//! DOM-free editing logic for the text-properties panel, kept out of the
//! Svelte component (and its `$state`/DOM concerns) so it is unit-testable in
//! the node (no-DOM) vitest env. The component owns element refs, focus, and
//! the keyed `{#each}`; this module owns the engine round-trips.

import type { Engine } from '../engine/protocol';

export type Rgba = [number, number, number, number];

/** The text-block style fields, in the engine's snake_case wire shape. */
export interface StyleFields {
    font_family?: string;
    size?: number;
    weight?: number;
    italic?: boolean;
    align?: string;
    color?: Rgba;
}

/** Full style baked into a brand-new text block by `add_text`. */
export interface FullStyle {
    font_family: string;
    size: number;
    weight: number;
    italic: boolean;
    align: string;
}

export interface Placement {
    x: number;
    y: number;
    anchorLayerId: number | null;
}

/** The slice of the app the editor logic drives. `app` satisfies it
 *  structurally; tests pass a fake. */
export interface EditorHost {
    engine: Engine | null;
    selectLayer(id: number | null): void;
    refreshLayerTree(): Promise<void>;
    requestFrame(): void;
}

/** Deferred create — the text layer is born on the first typed character of a
 *  pending placement, so an abandoned placement never makes a layer.
 *
 *  Sequencing is load-bearing: we resolve the new object id and flush any
 *  characters typed *during* the awaits via `set_text_content` BEFORE selecting
 *  the layer. The selection triggers the panel's bound-mode refetch, so the
 *  refetch already sees the latest content and the in-progress textarea is
 *  never clobbered. Returns the new `{ layerId, objectId, latest }`, or null on
 *  failure. */
export async function createTextFromPending(
    host: EditorHost,
    placement: Placement,
    content: string,
    style: FullStyle,
    color: Rgba,
    latest: () => string,
): Promise<{ layerId: number; objectId: number; latest: string } | null> {
    const engine = host.engine;
    if (!engine) return null;
    const res = await engine.send<{ id: number; object: number }>('add_text', {
        content,
        x: placement.x,
        y: placement.y,
        size: style.size,
        font_family: style.font_family,
        align: style.align,
        italic: style.italic,
        weight: style.weight,
        color,
        anchor: placement.anchorLayerId ?? -1,
    });
    if (!res || typeof res.id !== 'number' || typeof res.object !== 'number') return null;
    const layerId = res.id;
    const objectId = res.object;
    const latestVal = latest();
    if (latestVal !== content) {
        engine.post('set_text_content', { id: layerId, object: objectId, content: latestVal });
    }
    await host.refreshLayerTree();
    host.selectLayer(layerId);
    host.requestFrame();
    return { layerId, objectId, latest: latestVal };
}

// --- rAF-coalesced content dispatch ---------------------------------------
//
// Each kept keystroke re-shapes (parley) + re-rasters (Vello) the object. To
// keep a fast typist from triggering one re-shape per character, content writes
// are coalesced to at most one dispatch per animation frame (the latest value
// wins), keyed by object id.

const pendingContent = new Map<number, { host: EditorHost; layer: number; content: string }>();
let flushScheduled = false;

function scheduleFlush() {
    if (flushScheduled) return;
    flushScheduled = true;
    const run = () => {
        flushScheduled = false;
        flushTextContent();
    };
    // rAF in the browser; a microtask fallback keeps the no-DOM test env sane.
    if (typeof requestAnimationFrame !== 'undefined') {
        requestAnimationFrame(run);
    } else {
        void Promise.resolve().then(run);
    }
}

/** Queue a content write for `object` on `layer`, coalesced to one dispatch per
 *  frame. */
export function queueTextContent(host: EditorHost, layer: number, object: number, content: string) {
    pendingContent.set(object, { host, layer, content });
    scheduleFlush();
}

/** Dispatch every queued content write now. The rAF tick calls this; callers
 *  also call it synchronously on blur / tool-switch so the final keystroke is
 *  never dropped. (Document undo can only fire while the textarea is blurred —
 *  global hotkeys are suppressed over a focused TEXTAREA — so the blur flush is
 *  also the "flush before undo" guarantee.) */
export function flushTextContent() {
    if (pendingContent.size === 0) return;
    const hosts = new Set<EditorHost>();
    for (const [object, e] of pendingContent) {
        e.host.engine?.post('set_text_content', { id: e.layer, object, content: e.content });
        hosts.add(e.host);
    }
    pendingContent.clear();
    hosts.forEach((h) => h.requestFrame());
}

/** Map an engine style field to its `TextSession` default property, or null for
 *  fields (color) that aren't a placement default. */
const SESSION_DEFAULT_KEY: Record<string, string> = {
    font_family: 'fontFamily',
    size: 'size',
    weight: 'weight',
    italic: 'italic',
    align: 'align',
};

/** Mirror a style edit into the placement defaults so the next new block reuses
 *  the latest style. Skips `color` (never a default — new blocks take the
 *  current foreground). */
export function applyStyleDefaults(defaults: Record<string, unknown>, fields: StyleFields) {
    for (const [k, v] of Object.entries(fields)) {
        const key = SESSION_DEFAULT_KEY[k];
        if (key) defaults[key] = v;
    }
}

/** Apply a live style/color edit to a bound text object: post `set_text_style`
 *  and mirror the change into the placement defaults. */
export function dispatchStyle(
    host: EditorHost,
    layer: number,
    object: number,
    fields: StyleFields,
    defaults: Record<string, unknown>,
) {
    applyStyleDefaults(defaults, fields);
    host.engine?.post('set_text_style', { id: layer, object, ...fields });
    host.requestFrame();
}

/** Whether an incoming engine content value should re-seed the (uncontrolled)
 *  textarea. True only for an *external* change (undo/redo) — a self-echo of
 *  what we last sent leaves the field untouched, preserving the caret. */
export function shouldReseed(incoming: string, lastSent: string | undefined): boolean {
    return incoming !== lastSent;
}

// --- color <-> hex --------------------------------------------------------

export function rgbaToHex(c: Rgba): string {
    const h = (n: number) => n.toString(16).padStart(2, '0');
    return `#${h(c[0])}${h(c[1])}${h(c[2])}`;
}

export function hexToRgb(hex: string): [number, number, number] | null {
    const m = /^#?([0-9a-fA-F]{6})$/.exec(hex.trim());
    if (!m) return null;
    const v = parseInt(m[1], 16);
    return [(v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff];
}
