<script lang="ts">
    import Modal from './Modal.svelte';
    import { about } from '../state/about.svelte';
    import { darklyVersion } from '../version';

    // Relative to Vite's base ('./') so it resolves both at a web root and from
    // file:// in the packaged desktop bundle.
    const bannerSrc = `${import.meta.env.BASE_URL}darkly-banner.png`;

    const WEBSITE = 'https://darkly.art';
    const GITHUB = 'https://github.com/darkly-art/darkly';
</script>

<Modal bind:open={about.open} title="About Darkly" size="sm">
    <div class="about">
        <img class="banner" src={bannerSrc} alt="Darkly" />
        <p class="tagline">
            A GPU-native paint program written in Rust. Runs offline, no login,
            free and open source forever.
        </p>
        <p class="version">{darklyVersion}</p>

        <div class="links">
            <a class="link" href={WEBSITE} target="_blank" rel="noopener noreferrer">
                <i class="fa-solid fa-globe"></i>
                <span>Website</span>
                <i class="fa-solid fa-arrow-up-right-from-square external"></i>
            </a>
            <a class="link" href={GITHUB} target="_blank" rel="noopener noreferrer">
                <i class="fa-brands fa-github"></i>
                <span>GitHub</span>
                <i class="fa-solid fa-arrow-up-right-from-square external"></i>
            </a>
        </div>

        <p class="license">Licensed under AGPL-3.0-or-later</p>
    </div>
</Modal>

<style>
    .about {
        display: flex;
        flex-direction: column;
        align-items: center;
        text-align: center;
        gap: 12px;
    }

    .banner {
        width: min(280px, 70%);
        height: auto;
    }

    .tagline {
        margin: 0;
        max-width: 36ch;
        font-size: 13px;
        line-height: 1.5;
        color: var(--text-muted);
    }

    .version {
        margin: 0;
        font-family: var(--font-mono, monospace);
        font-size: 13px;
        color: var(--text);
    }

    .links {
        display: flex;
        gap: 10px;
        margin-top: 4px;
    }

    .link {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 8px 14px;
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        color: var(--text);
        font-size: 13px;
        text-decoration: none;
        transition: background 0.1s, border-color 0.1s;
    }

    .link:hover {
        background: var(--bg-hover);
    }

    .link i { color: var(--text-muted); }
    .link .external { font-size: 10px; }

    .license {
        margin: 4px 0 0;
        font-size: 11px;
        color: var(--text-muted);
    }
</style>
