/**
 * Reactive UI theme state.
 *
 * Owns the `dark` | `light` choice, applies it to the document body, and
 * keeps WASM in sync with the colors used for baking preset thumbnails —
 * so the brush picker grid looks consistent regardless of what paint
 * color the user is currently painting with.
 */
import { app } from './app.svelte';

export type ThemeName = 'dark' | 'light';

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

function initialThemeFromDom(): ThemeName {
    if (typeof document === 'undefined') return 'dark';
    return document.body.classList.contains('light') ? 'light' : 'dark';
}

class ThemeState {
    current = $state<ThemeName>(initialThemeFromDom());

    /** Apply `name` to the document body and push colors to WASM. */
    set(name: ThemeName) {
        this.current = name;
        if (typeof document !== 'undefined') {
            document.body.classList.remove('dark', 'light');
            document.body.classList.add(name);
        }
        this.pushToWasm();
    }

    /** Push the current theme's preview colors to WASM.
     *  Safe to call before `app.handle` is ready — no-op in that case. */
    pushToWasm() {
        if (!app.handle) return;
        const colors = PREVIEW_COLORS[this.current];
        app.handle.set_preview_theme(colors.fg, colors.bg);
    }
}

export const theme = new ThemeState();
