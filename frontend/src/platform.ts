// Last reviewed: 2026-04

export type Os =
    | 'windows'
    | 'macos'
    | 'linux'
    | 'ios'
    | 'android'
    | 'chromeos'
    | 'unknown';

export type Browser = 'chromium' | 'firefox' | 'safari' | 'unknown';

export interface Platform {
    os: Os;
    browser: Browser;
}

interface UADataBrand {
    brand: string;
    version: string;
}
interface UAData {
    platform?: string;
    brands?: UADataBrand[];
}

const CHROMIUM_BRANDS = new Set([
    'Chromium',
    'Google Chrome',
    'Microsoft Edge',
    'Brave',
    'Opera',
    'Vivaldi',
]);

export function detectPlatform(
    nav: { userAgent?: string; platform?: string; maxTouchPoints?: number; userAgentData?: UAData } = navigator,
): Platform {
    const ua = nav.userAgent ?? '';
    const uad = nav.userAgentData;
    const plat = nav.platform ?? '';
    const touchPoints = nav.maxTouchPoints ?? 0;

    const os = detectOs(ua, uad, plat, touchPoints);
    const browser = detectBrowser(os, ua, uad);

    return { os, browser };
}

function detectOs(ua: string, uad: UAData | undefined, plat: string, touchPoints: number): Os {
    // iOS first — iPadOS lies as MacIntel and all iOS browsers are WebKit.
    if (/iPhone|iPod/.test(ua)) return 'ios';
    if (plat === 'MacIntel' && touchPoints > 1) return 'ios';
    if (/iPad/.test(ua)) return 'ios';

    const uadPlat = uad?.platform ?? '';
    if (uadPlat === 'Windows' || /Windows/.test(ua)) return 'windows';
    if (uadPlat === 'macOS' || /Mac OS X|Macintosh/.test(ua)) return 'macos';
    if (uadPlat === 'Android' || /Android/.test(ua)) return 'android';
    if (uadPlat === 'Chrome OS' || /CrOS/.test(ua)) return 'chromeos';
    if (uadPlat === 'Linux' || /Linux/.test(ua)) return 'linux';

    return 'unknown';
}

function detectBrowser(os: Os, ua: string, uad: UAData | undefined): Browser {
    // All iOS browsers are WebKit under the hood — same enable path.
    if (os === 'ios') return 'safari';

    if (uad?.brands?.some(b => CHROMIUM_BRANDS.has(b.brand))) return 'chromium';

    // Firefox first: its UA contains "Firefox" and nothing that would trip
    // the Chromium check below.
    if (/Firefox|FxiOS/.test(ua)) return 'firefox';

    // Desktop Chromium UAs contain both "Chrome" and "Safari"; desktop
    // Safari contains "Safari" but not "Chrome"/"Chromium".
    if (/Chrome|Chromium|CriOS|EdgiOS/.test(ua)) return 'chromium';
    if (/Safari/.test(ua)) return 'safari';

    return 'unknown';
}
