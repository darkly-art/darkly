import { describe, it, expect } from 'vitest';
import { isVisibleWhen } from '../visibleWhen';
import { visibleParams } from '../../ui/params/paramSchema';
import { param } from '../../ui/params/__tests__/paramFixture';

describe('isVisibleWhen', () => {
    const values: Record<string, unknown> = { style: 1 };
    const valueOf = (name: string) => values[name];

    it('shows a control with no rule', () => {
        expect(isVisibleWhen(null, valueOf)).toBe(true);
        expect(isVisibleWhen(undefined, valueOf)).toBe(true);
    });

    it('shows a control whose selector value is allowed', () => {
        expect(isVisibleWhen(['style', [0, 1]], valueOf)).toBe(true);
    });

    it('hides a control whose selector value is not allowed', () => {
        expect(isVisibleWhen(['style', [2]], valueOf)).toBe(false);
    });

    it('shows a control whose selector is absent', () => {
        expect(isVisibleWhen(['missing', [2]], valueOf)).toBe(true);
    });
});

describe('visibleParams', () => {
    const style = (value: number | null) =>
        param({ kind: 'enum', name: 'style', default: 0, value, options: ['Smooth', 'Blob', 'Superformula'] });
    const ripple = param({ kind: 'float', name: 'ripple', default: 0, visible_when: ['style', [0]] });
    const wobble = param({ kind: 'float', name: 'wobble', default: 0.35, visible_when: ['style', [1]] });
    const m = param({ kind: 'int', name: 'm', default: 5, visible_when: ['style', [2]] });

    it("keeps the selector plus exactly the current style's params", () => {
        const names = (v: number | null) => visibleParams([style(v), ripple, wobble, m]).map((p) => p.name);
        expect(names(null)).toEqual(['style', 'ripple']);
        expect(names(1)).toEqual(['style', 'wobble']);
        expect(names(2)).toEqual(['style', 'm']);
    });

    it('resolves a rule against the whole schema when rendering a subset', () => {
        const all = [style(2), ripple, m];
        expect(visibleParams([ripple, m], all).map((p) => p.name)).toEqual(['m']);
    });
});
