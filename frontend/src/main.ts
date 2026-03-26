import './themes/dark.css';
import './themes/light.css';
import './styles/reset.css';
import './styles/tokens.css';
import App from './App.svelte';
import { mount } from 'svelte';

const app = mount(App, {
    target: document.getElementById('app')!,
});

export default app;
