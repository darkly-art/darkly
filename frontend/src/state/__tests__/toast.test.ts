import { describe, it, expect, beforeEach } from 'vitest';
import { toast } from '../toast.svelte';

describe('toast', () => {
    beforeEach(() => {
        for (const t of [...toast.toasts]) toast.dismiss(t.id);
    });

    it('collapses a repeated message instead of stacking it', () => {
        toast.show('warning', 'cannot paint here');
        toast.show('warning', 'cannot paint here');
        toast.show('warning', 'cannot paint here');

        expect(toast.toasts.map((t) => t.message)).toEqual(['cannot paint here']);
    });

    it('keeps distinct messages side by side', () => {
        toast.show('warning', 'cannot paint here');
        toast.show('warning', 'layer is locked');
        toast.show('error', 'cannot paint here');

        expect(toast.toasts).toHaveLength(3);
    });
});
