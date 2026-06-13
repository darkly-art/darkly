import { describe, it, expect, vi } from 'vitest';
import { backdropDismiss } from '../backdropDismiss';

// jsdom isn't available in this project, so we exercise the action against a
// fake node that records its listeners and lets the test dispatch
// `{ target }` events. That's enough to pin the behaviour that matters: it
// dismisses only when the *press* originated on the backdrop, not merely when
// the release lands there (the text-selection-drag bug).

function fakeNode() {
    const handlers: Record<string, (e: { target: unknown }) => void> = {};
    const removed: string[] = [];
    return {
        node: {
            addEventListener: (type: string, h: (e: { target: unknown }) => void) => {
                handlers[type] = h;
            },
            removeEventListener: (type: string) => {
                removed.push(type);
            },
        } as unknown as HTMLElement,
        handlers,
        removed,
        // Dispatch with a target that is the node itself unless `child` is true.
        press(child = false) {
            handlers.pointerdown({ target: child ? {} : this.node });
        },
        click(child = false) {
            handlers.click({ target: child ? {} : this.node });
        },
    };
}

describe('backdropDismiss', () => {
    it('dismisses when press and release both land on the backdrop', () => {
        const f = fakeNode();
        const cb = vi.fn();
        backdropDismiss(f.node, cb);
        f.press();
        f.click();
        expect(cb).toHaveBeenCalledTimes(1);
    });

    it('does NOT dismiss when the press started on inner content (the bug)', () => {
        const f = fakeNode();
        const cb = vi.fn();
        backdropDismiss(f.node, cb);
        f.press(true); // mousedown inside the modal (e.g. starting a selection)
        f.click(); //     mouseup released over the backdrop
        expect(cb).not.toHaveBeenCalled();
    });

    it('does NOT dismiss when the release lands on inner content', () => {
        const f = fakeNode();
        const cb = vi.fn();
        backdropDismiss(f.node, cb);
        f.press(); //      mousedown on the backdrop
        f.click(true); //  mouseup over content
        expect(cb).not.toHaveBeenCalled();
    });

    it('destroy unbinds both listeners', () => {
        const f = fakeNode();
        const { destroy } = backdropDismiss(f.node, () => {});
        destroy();
        expect(f.removed).toContain('pointerdown');
        expect(f.removed).toContain('click');
    });

    it('update swaps the callback used by a later dismissal', () => {
        const f = fakeNode();
        const first = vi.fn();
        const second = vi.fn();
        const { update } = backdropDismiss(f.node, first);
        update(second);
        f.press();
        f.click();
        expect(first).not.toHaveBeenCalled();
        expect(second).toHaveBeenCalledTimes(1);
    });
});
