import './themes/dark.css';
import './themes/light.css';
import './styles/reset.css';
import './styles/tokens.css';
import App from './App.svelte';
import GpuErrorPage from './ui/GpuErrorPage.svelte';
import { mount } from 'svelte';
import { checkGpu } from './gpu';
import { detectPlatform } from './platform';
import { suppressButtonKeyboardFocus } from './lib/suppressButtonKeyboardFocus';

suppressButtonKeyboardFocus();

const target = document.getElementById('app')!;

async function boot() {
    const check = await checkGpu();
    target.replaceChildren();

    if (check.ok) {
        return mount(App, { target });
    }

    const platform = detectPlatform();
    return mount(GpuErrorPage, {
        target,
        props: { failure: check, platform },
    });
}

const app = boot();

export default app;
