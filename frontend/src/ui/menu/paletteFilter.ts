import type { ActionRegistration } from '../../actions/registry';

/** Actions eligible for the command palette: everything except `type:'hold'`
 *  actions (drag-bound primitives like `sampleColor` / `brushSizeAdjust`
 *  that make no sense to fire from a list). */
export function paletteActions(regs: ActionRegistration[]): ActionRegistration[] {
    return regs.filter(r => r.type !== 'hold');
}

/**
 * Filter + rank palette actions for a query. Substring match over
 * name + description + menuPath + category; empty query returns all eligible
 * actions in registration order. Ranking is name-centric: exact name first,
 * then name-prefix, then name-substring, then matches that hit only the
 * description / path / category. Array#sort is stable, so ties keep
 * registration order.
 */
export function filterPalette(regs: ActionRegistration[], query: string): ActionRegistration[] {
    const eligible = paletteActions(regs);
    const q = query.trim().toLowerCase();
    if (!q) return eligible;

    const scored: { reg: ActionRegistration; score: number }[] = [];
    for (const reg of eligible) {
        const name = reg.displayName.toLowerCase();
        const haystack = [
            reg.displayName,
            reg.description ?? '',
            ...(reg.menuPath ?? []),
            reg.category,
        ].join(' ').toLowerCase();
        if (!haystack.includes(q)) continue;

        let score = 3;
        if (name === q) score = 0;
        else if (name.startsWith(q)) score = 1;
        else if (name.includes(q)) score = 2;
        scored.push({ reg, score });
    }

    scored.sort((a, b) => a.score - b.score);
    return scored.map(s => s.reg);
}
