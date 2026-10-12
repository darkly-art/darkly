import { describe, it, expect } from 'vitest';
import { detectPlatform } from '../platform';

const CHROMIUM_BRANDS = [{ brand: 'Not?A_Brand', version: '99' }, { brand: 'Chromium', version: '152' }];
const LINUX_CHROME_UA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.7977.130 Safari/537.36';

describe('detectPlatform', () => {
    it('detects the Darkly desktop app by its Electron UA token, despite Chromium brands', () => {
        const ua = LINUX_CHROME_UA.replace(' Safari/', ' Electron/44.4.5 Safari/');
        expect(detectPlatform({ userAgent: ua, userAgentData: { platform: 'Linux', brands: CHROMIUM_BRANDS } }))
            .toEqual({ os: 'linux', browser: 'electron' });
    });

    it('still detects plain Chromium by its brands', () => {
        expect(detectPlatform({ userAgent: LINUX_CHROME_UA, userAgentData: { platform: 'Linux', brands: CHROMIUM_BRANDS } }))
            .toEqual({ os: 'linux', browser: 'chromium' });
    });
});
