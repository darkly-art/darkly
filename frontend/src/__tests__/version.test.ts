import { describe, it, expect } from 'vitest';
import { darklyVersion } from '../version';

describe('darklyVersion', () => {
    it('is the raw git-describe string (tag-height-gsha), copyable verbatim', () => {
        // `git describe --tags --long` shape (also matches the `0.0.0-0-gunknown`
        // fallback): no decorative `+`/`·`, so it round-trips with the version
        // the Rust crate stamps into saved files.
        expect(darklyVersion).toMatch(/^.+-\d+-g[0-9a-zA-Z]+$/);
    });
});
