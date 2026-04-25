/**
 * Reactive UI theme state.
 *
 * Owns the `dark` | `light` | `system` choice, applies it to the document
 * body, and keeps WASM in sync with the colors used for baking preset
 * thumbnails — so the brush picker grid looks consistent regardless of the
 * paint color the user is currently painting with.
 *
 * Persistence flows through the unified config store (`ui.theme`) rather
 * than direct localStorage, so the Settings modal's Theme widget and the
 * hamburger shortcut both round-trip through the same place.
 */
import { app } from './app.svelte';
import { config } from '../config/store.svelte';

export type ThemeName = 'dark' | 'light';
export type ThemePreference = ThemeName | 'system';

/** Linear RGBA colors used by the live preview and preset thumbnails.
 *  Pure black/white for maximum contrast and to match each theme's
 *  canonical `--bg` (sRGB #000 for dark, #fff for light). */
const PREVIEW_COLORS: Record<ThemeName, { fg: Float32Array; bg: Float32Array }> = {
    dark: {
        fg: new Float32Array([1.0, 1.0, 1.0, 1.0]),
        bg: new Float32Array([0.0, 0.0, 0.0, 1.0]),
    },
    light: {
        fg: new Float32Array([0.0, 0.0, 0.0, 1.0]),
        bg: new Float32Array([1.0, 1.0, 1.0, 1.0]),
    },
};

function systemTheme(): ThemeName {
    if (typeof window === 'undefined' || !window.matchMedia) return 'dark';
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

class ThemeState {
    /** The user's stated preference (what's persisted). */
    preference = $state<ThemePreference>('dark');
    /** The concrete theme actually applied to the document. */
    current = $state<ThemeName>('dark');

    #mql: MediaQueryList | null = null;

    /** Sync from config. Called once on init and again whenever `ui.theme` changes. */
    syncFromConfig() {
        const raw = config.get('ui.theme');
        const pref: ThemePreference =
            raw === 'light' || raw === 'dark' || raw === 'system' ? raw : 'dark';
        this.preference = pref;
        this.current = pref === 'system' ? systemTheme() : pref;
        this.#applyToDom();
        this.pushToWasm();
        this.#ensureMqlListener();
    }

    /** User action: change the theme preference, persist it through config. */
    set(pref: ThemePreference) {
        config.set('ui.theme', pref);
        // syncFromConfig will run via the config.onChange subscriber below.
    }

    /** Push the current theme's preview colors to WASM.
     *  Safe to call before `app.handle` is ready — no-op in that case. */
    pushToWasm() {
        if (!app.handle) return;
        const colors = PREVIEW_COLORS[this.current];
        app.handle.set_preview_theme(colors.fg, colors.bg);
    }

    #applyToDom() {
        if (typeof document === 'undefined') return;
        document.body.classList.remove('dark', 'light');
        document.body.classList.add(this.current);
    }

    #ensureMqlListener() {
        if (this.#mql !== null || typeof window === 'undefined' || !window.matchMedia) return;
        this.#mql = window.matchMedia('(prefers-color-scheme: dark)');
        this.#mql.addEventListener('change', () => {
            if (this.preference === 'system') {
                this.current = systemTheme();
                this.#applyToDom();
                this.pushToWasm();
            }
        });
    }
}

export const theme = new ThemeState();

// Keep theme in sync whenever the config mutates (covers: initial load,
// applyPreset, direct user set via the Settings modal or hamburger menu).
config.onChange(() => theme.syncFromConfig());
