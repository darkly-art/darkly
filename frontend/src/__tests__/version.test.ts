import { describe, it, expect } from 'vitest';
import { parseVersion, formatVersion } from '../version';

describe('parseVersion', () => {
    it('splits `git describe --tags --long` output', () => {
        expect(parseVersion('v0.3.0-1-gf0c3ea9')).toEqual({
            tag: 'v0.3.0',
            commits: 1,
            sha: 'f0c3ea9',
        });
    });

    it('parses the build-time fallback', () => {
        expect(parseVersion('0.0.0-0-gunknown')).toEqual({
            tag: '0.0.0',
            commits: 0,
            sha: 'unknown',
        });
    });

    it('keeps dashes that belong to the tag', () => {
        expect(parseVersion('v1.2.0-rc1-5-gabc1234')).toEqual({
            tag: 'v1.2.0-rc1',
            commits: 5,
            sha: 'abc1234',
        });
    });
});

describe('formatVersion', () => {
    it('shows just the tag at a release (commit height 0)', () => {
        expect(formatVersion('v0.3.0-0-gf0c3ea9')).toBe('v0.3.0');
    });

    it('appends the commit height when ahead of the tag', () => {
        const out = formatVersion('v0.3.0-1-gf0c3ea9');
        expect(out).toContain('v0.3.0');
        expect(out).toContain('+1');
    });
});
