import type { PrefInfo, SectionInfo } from './schema';

/**
 * Validate stored user overrides against the live schema. Unknown keys are
 * dropped; type-mismatched values are dropped; numerics out of range are
 * clamped. Returns the cleaned override set. The caller is responsible for
 * persisting the cleaned set back if anything changed.
 */
export function validateOverrides(
    sections: SectionInfo[],
    overrides: Record<string, unknown>,
): { cleaned: Record<string, unknown>; changed: boolean } {
    const byKey = new Map<string, PrefInfo>();
    for (const section of sections) {
        for (const pref of section.prefs) byKey.set(pref.key, pref);
    }

    const cleaned: Record<string, unknown> = {};
    let changed = false;

    for (const [key, value] of Object.entries(overrides)) {
        const pref = byKey.get(key);
        if (!pref) {
            console.warn(`[config] Dropping unknown pref key: ${key}`);
            changed = true;
            continue;
        }
        const coerced = coerce(pref, value);
        if (coerced === DROP) {
            console.warn(
                `[config] Dropping pref ${key}: value ${JSON.stringify(value)} does not match kind ${pref.kind}`,
            );
            changed = true;
            continue;
        }
        if (coerced !== value) changed = true;
        cleaned[key] = coerced;
    }

    return { cleaned, changed };
}

const DROP = Symbol('drop');

function coerce(pref: PrefInfo, value: unknown): unknown | typeof DROP {
    switch (pref.kind) {
        case 'bool':
            return typeof value === 'boolean' ? value : DROP;
        case 'str':
            return typeof value === 'string' ? value : DROP;
        case 'enum': {
            if (typeof value !== 'string') return DROP;
            const ok = pref.options?.some(([k]) => k === value) ?? false;
            return ok ? value : DROP;
        }
        case 'int':
        case 'float': {
            if (typeof value !== 'number' || !Number.isFinite(value)) return DROP;
            let v = value;
            if (pref.kind === 'int') v = Math.trunc(v);
            if (pref.min !== undefined && v < pref.min) v = pref.min;
            if (pref.max !== undefined && v > pref.max) v = pref.max;
            return v;
        }
    }
}
