// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';

/**
 * Where the explorer lands when it opens.
 *
 * The reported failure was a reopen landing on an unrelated pack. Its cause is
 * a scroll-anchoring adjustment Chromium applies to a restored offset when a
 * lead spacer sized against the scrollport measures zero in the first layout
 * after the dialog is re-attached. jsdom has no layout, no `display: none`
 * semantics and no scroll anchoring, so that drift cannot be reproduced here
 * and is not what these tests pin.
 *
 * What they pin is the behaviour that makes the drift unobservable: an open
 * places the list explicitly, so wherever the previous session left it, and
 * whatever the browser restored, is overwritten. That is a feature test for the
 * landing, not a regression test for the CSS defect.
 *
 * Layout is canned, because a placement test needs a layout it controls and
 * jsdom supplies none. All rects read zero while the dialog is closed, which is
 * what an undisplayed scrollport reports in a browser.
 */

const PORT = 400;
const LEAD = PORT * 0.5; // FOCUS_LINE
const SECTION_H = 100;
const SECTION_PITCH = 112;
const CARD_H = 48;
const CARD_PITCH = 52;

/** Where the list is left before the close: the focus line sits in Dry Media. */
const LEFT_AT = 250;
/** `scrollTopForSection(0)`: Recents centred on the focus line. */
const RECENTS_TOP = LEAD + (SECTION_H - PORT) / 2;

function brush(id: string, name: string) {
    return {
        id,
        name,
        author: '',
        description: '',
        tags: [] as string[],
        icon: null,
        can_edit: false,
    };
}

function pack(id: string, name: string, members: string[]) {
    return {
        id,
        name,
        description: '',
        icon: 'fa6-solid:folder',
        palette: { chroma: '#888', refraction: '#999', surface: '#111' },
        members,
        can_edit_members: false,
        can_edit_identity: false,
    };
}

// Four brushes over three packs, with `pencil` in recents, so the rendered
// groups are Recents, Basic, Dry Media, Wet Media: section 0 is Recents and the
// last pack is a wet one, the shape the reported case had.
const library = {
    brushes: [
        brush('pencil', 'pencil'),
        brush('charcoal', 'charcoal'),
        brush('watercolor', 'rough_watercolor'),
        brush('airbrush', 'airbrush'),
    ],
    packs: [
        pack('basic', 'Basic', ['airbrush']),
        pack('dry_media', 'Dry Media', ['pencil', 'charcoal']),
        pack('wet_media', 'Wet Media', ['watercolor']),
    ],
};

const brushGraph = { activeBrush: 'pencil', loadBrush: vi.fn() };

vi.mock('../../../state/brush_library.svelte', () => ({ brushLibrary: library }));
vi.mock('../../../state/brush_graph.svelte', () => ({ brushGraph }));
vi.mock('../../../state/recents.svelte', () => ({
    recentBrushes: { items: ['pencil'], use: vi.fn(), retain: vi.fn() },
}));
// The real strip reads the engine's baked PNG cache; there is no engine here.
vi.mock('../../brush_library/BrushPreviewStrip.svelte', async () => ({
    default: (await import('./PreviewStripStub.svelte')).default,
}));

const Harness = (await import('./BrushExplorerHarness.svelte')).default;

let target: HTMLElement;
let instance: Record<string, unknown> | undefined;
let frames: FrameRequestCallback[] = [];

/** The dialog is the authority on whether there is a layout to measure. */
function displayed(): boolean {
    return !!target?.querySelector('dialog')?.open;
}

function listEl(): HTMLElement {
    return target.querySelector('.list') as HTMLElement;
}

function wheelEl(): HTMLElement {
    return target.querySelector('.pack-wheel') as HTMLElement;
}

function cards(): HTMLElement[] {
    return Array.from(target.querySelectorAll('.pack-card'));
}

beforeAll(() => {
    // jsdom implements none of these; the explorer only needs them to behave.
    HTMLDialogElement.prototype.showModal = function () {
        this.open = true;
    };
    HTMLDialogElement.prototype.close = function () {
        this.open = false;
    };
    // jsdom has no scrolling at all, so `scrollTo` would be a no-op and the
    // placement under test would be invisible. Clamped as a real scrollport is.
    Element.prototype.scrollTo = function (opts?: ScrollToOptions | number) {
        const top = typeof opts === 'number' ? opts : (opts?.top ?? this.scrollTop);
        const max = Math.max(0, this.scrollHeight - PORT);
        this.scrollTop = Math.max(0, Math.min(top, max));
    };

    // A synthetic layout: 400 px ports side by side, sections and cards on a
    // fixed pitch, each offset by its own pane's scroll. Zero while closed.
    Element.prototype.getBoundingClientRect = function (): DOMRect {
        const r = (top: number, height: number, left: number, width: number) =>
            ({ top, height, bottom: top + height, left, width, right: left + width, x: left, y: top })  as DOMRect;
        if (!displayed()) return r(0, 0, 0, 0);
        const el = this as HTMLElement;
        if (el.classList.contains('explorer')) return r(0, PORT, 0, 800);
        if (el.classList.contains('list')) return r(0, PORT, 300, 500);
        if (el.classList.contains('pack-wheel')) return r(0, PORT, 0, 300);
        if (el.classList.contains('pack-card')) {
            const i = cards().indexOf(el);
            return r(i * CARD_PITCH - wheelEl().scrollTop, CARD_H, 0, 300);
        }
        if (el.tagName === 'SECTION') {
            const sections = Array.from(listEl().querySelectorAll(':scope > section'));
            const i = sections.indexOf(el);
            return r(LEAD + i * SECTION_PITCH - listEl().scrollTop, SECTION_H, 300, 500);
        }
        return r(0, 0, 0, 0);
    };

    // `scrollHeight` is a getter on the prototype in jsdom and always 0.
    const contentHeight = (el: HTMLElement) => {
        if (!displayed()) return 0;
        if (el.classList.contains('list')) {
            const n = el.querySelectorAll(':scope > section').length;
            return LEAD * 2 + n * SECTION_PITCH;
        }
        if (el.classList.contains('pack-wheel')) return cards().length * CARD_PITCH;
        return 0;
    };
    Object.defineProperty(Element.prototype, 'scrollHeight', {
        configurable: true,
        get(this: HTMLElement) {
            return contentHeight(this);
        },
    });
});

beforeEach(() => {
    frames = [];
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
        frames.push(cb);
        return frames.length;
    });
    vi.stubGlobal('cancelAnimationFrame', () => {});
    target = document.createElement('div');
    document.body.append(target);
    instance = mount(Harness, { target, props: {} });
    flushSync();
});

afterEach(() => {
    if (instance) unmount(instance);
    target.remove();
    vi.unstubAllGlobals();
});

/** Run every frame currently queued, then let Svelte settle. */
function frame() {
    const queued = frames;
    frames = [];
    for (const cb of queued) cb(performance.now());
    flushSync();
}

async function setOpen(open: boolean) {
    (instance as { setOpen(v: boolean): void }).setOpen(open);
    flushSync();
    await tick();
    flushSync();
}

describe('brush explorer open placement', () => {
    it('lands Recents on the focus line whatever the list was left at', async () => {
        await setOpen(true);
        frame();
        frame();

        // The first open is placed too, rather than sitting at 0 with Recents'
        // top edge on the line instead of its centre.
        expect(listEl().scrollTop).toBe(RECENTS_TOP);

        // Leave it somewhere else entirely, as picking a brush from a pack does.
        listEl().scrollTop = LEFT_AT;
        frame();
        expect(cards().findIndex(c => c.getAttribute('aria-current') === 'true')).toBe(2);

        await setOpen(false);
        await setOpen(true);
        frame();
        frame();

        expect(listEl().scrollTop).toBe(RECENTS_TOP);
        expect(cards().findIndex(c => c.getAttribute('aria-current') === 'true')).toBe(0);
    });

    it('schedules no frames while closed', async () => {
        await setOpen(true);
        frame();
        await setOpen(false);
        frames = [];

        // A scroll event on a closed explorer's pane must not wake the loop.
        // Asserted through a real event rather than through a hidden measure,
        // so it tests the guard rather than the canned zero-height layout.
        listEl().dispatchEvent(new Event('scroll'));
        flushSync();

        expect(frames.length).toBe(0);
    });
});
