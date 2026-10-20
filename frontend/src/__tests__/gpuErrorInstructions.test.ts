import { describe, it, expect } from 'vitest';
import { instructionsFor, type Instructions } from '../gpuErrorInstructions';
import type { Browser, Os } from '../platform';

const text = (i: Instructions) => JSON.stringify(i);

describe('gpuErrorInstructions', () => {
    it('gives the desktop app on Linux no chrome:// flags it cannot open', () => {
        const linux = instructionsFor({ browser: 'electron', os: 'linux' });
        expect(linux.title).toBe('Darkly desktop on Linux');
        expect(text(linux)).not.toContain('chrome://');
    });

    // The flag only ever turned a missing hardware adapter into a software
    // one, which the GPU check rejects anyway.
    it.each<[Browser, Os]>([
        ['chromium', 'windows'],
        ['chromium', 'macos'],
        ['chromium', 'linux'],
        ['electron', 'windows'],
        ['electron', 'macos'],
        ['electron', 'linux'],
    ])('never suggests enable-unsafe-webgpu for %s on %s', (browser, os) => {
        expect(text(instructionsFor({ browser, os }))).not.toContain('enable-unsafe-webgpu');
    });

    it('points Chromium on Linux at the Vulkan flag', () => {
        const urls = instructionsFor({ browser: 'chromium', os: 'linux' }).flags.map((f) => f.url);
        expect(urls).toContain('chrome://flags/#enable-vulkan');
    });
});
