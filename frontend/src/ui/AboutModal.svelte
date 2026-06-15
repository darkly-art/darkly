<script lang="ts">
    import Modal from './Modal.svelte';
    import Icon from '../icons/Icon.svelte';
    import { about } from '../state/about.svelte';
    import { darklyVersion } from '../version';
    import { links } from '../links';

    // Relative to Vite's base ('./') so it resolves both at a web root and from
    // file:// in the packaged desktop bundle.
    const bannerSrc = `${import.meta.env.BASE_URL}darkly-banner.png`;

    let copied = $state(false);
    let copyTimer: ReturnType<typeof setTimeout> | null = null;

    async function copyVersion() {
        try {
            await navigator.clipboard.writeText(darklyVersion);
            copied = true;
            if (copyTimer) clearTimeout(copyTimer);
            copyTimer = setTimeout(() => { copied = false; }, 1500);
        } catch {
            // Clipboard unavailable — the text is still selectable to copy by hand.
        }
    }
</script>

<Modal bind:open={about.open} title="About Darkly" size="sm">
    <div class="about">
        <img class="banner" src={bannerSrc} alt="Darkly" />
        <div class="version-row">
            <span class="version-label">Version:</span>
            <button
                class="version-copy"
                onclick={copyVersion}
                title="Click to copy"
                aria-label="Copy version {darklyVersion}"
            >
                <code>{darklyVersion}</code>
                <Icon name={copied ? 'fa6-solid:check' : 'fa6-solid:copy'} class={copied ? 'copied' : ''} />
            </button>
        </div>

        <div class="links">
            <a class="link" href={links.website} target="_blank" rel="noopener noreferrer">
                <Icon name="fa6-solid:globe" />
                <span>Website</span>
                <Icon name="fa6-solid:arrow-up-right-from-square" class="external" />
            </a>
            <a class="link" href={links.github} target="_blank" rel="noopener noreferrer">
                <Icon name="fa6-brands:github" />
                <span>GitHub</span>
                <Icon name="fa6-solid:arrow-up-right-from-square" class="external" />
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

    .version-row {
        display: flex;
        align-items: center;
        gap: 8px;
        font-size: 13px;
    }

    .version-label {
        color: var(--text-muted);
    }

    .version-copy {
        display: inline-flex;
        align-items: center;
        gap: 8px;
        padding: 4px 8px;
        background: none;
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        color: var(--text);
        cursor: pointer;
        font-family: var(--font-mono, monospace);
        font-size: 13px;
        transition: background 0.1s, border-color 0.1s;
    }

    .version-copy:hover {
        background: var(--bg-hover);
    }

    .version-copy code {
        font-family: inherit;
    }

    .version-copy :global(svg) {
        font-size: 11px;
        color: var(--text-muted);
    }

    .version-copy :global(.copied) {
        color: var(--accent);
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

    .link :global(svg) { color: var(--text-muted); }
    .link :global(.external) { font-size: 10px; }

    .license {
        margin: 4px 0 0;
        font-size: 11px;
        color: var(--text-muted);
    }
</style>
