import './themes/dark.css';
import './themes/light.css';
import './styles/reset.css';
import './styles/tokens.css';
import './styles/tool-strip-dock.css';
import './styles/drop-indicator.css';
import App from './App.svelte';
import BrushInspector from './ui/brush_inspector/BrushInspector.svelte';
import GpuErrorPage from './ui/GpuErrorPage.svelte';
import { mount } from 'svelte';
import { checkGpu, reportToHost, type HostVerdict } from './gpu';
import { detectPlatform } from './platform';
import { suppressButtonKeyboardFocus } from './lib/suppressButtonKeyboardFocus';
import { strokeRecorder } from './lib/strokeRecorder';
import { registerPwa } from './pwa';

suppressButtonKeyboardFocus();
strokeRecorder.init();

const target = document.getElementById('app')!;

async function boot() {
    // Brush inspector is a self-contained dev page: no GPU init, no engine
    // boot. Reach it via `?brush-inspect`.
    if (new URLSearchParams(window.location.search).has('brush-inspect')) {
        target.replaceChildren();
        return mount(BrushInspector, { target });
    }

    const check = await checkGpu();
    const host = window.electronAPI?.gpu;
    const verdict: HostVerdict = host ? await reportToHost(check, host) : { verdict: 'continue' };
    // The host is restarting the app; mounting anything would only flash.
    if (verdict.verdict === 'relaunching') return;
    target.replaceChildren();

    if (check.ok) {
        // Register the service worker only on the full-app path: the update
        // prompt renders via <Toast /> inside App, and a no-op in dev (SW
        // disabled in vite.config.ts) keeps this harmless during `vite`.
        registerPwa();
        return mount(App, { target });
    }

    const platform = detectPlatform();
    return mount(GpuErrorPage, {
        target,
        props: {
            failure: check,
            platform,
            diagnostics: verdict.verdict === 'exhausted' ? verdict.diagnostics : undefined,
        },
    });
}

const app = boot();

export default app;
