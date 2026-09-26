// Electron Forge configuration.
//
// Plain CommonJS (not TypeScript) so the Forge CLI can load it without an
// extra ts-node loader.

const path = require('path');

// Product metadata has one home, crates/darkly/product.yaml, and reaches
// non-Rust consumers through this generated file. `cargo sync-docs` writes it
// and the test suite fails when it is stale, so the maker fields below cannot
// drift from the desktop entry and the AppStream metainfo the way three
// hand-written copies of the category list did.
const app = require('../packaging/app.json');


// macOS code signing + notarization, enabled only when the CI signing step has
// provisioned credentials (see the "Sign and notarize (macOS)" job step).
//
// Gated on DARKLY_MAC_SIGN so that:
//   - Linux/Windows builds never touch this (codesign is macOS-only).
//   - PR / fork builds with no secrets still produce an unsigned bundle instead
//     of failing the whole job.
//
// osxSign is left as `{}`: @electron/osx-sign's defaults (hardened runtime + the
// stock entitlements that work for direct distribution) are correct for a
// Developer ID Application cert. It auto-discovers the identity from the
// keychain the CI step imported it into.
const macSign = process.env.DARKLY_MAC_SIGN === '1'
    ? {
          osxSign: {},
          osxNotarize: {
              // App Store Connect API key (despite the name, this is the
              // general-purpose credential notarytool uses for non-App-Store
              // notarization). Paths/IDs are exported by the CI signing step.
              appleApiKey: process.env.APPLE_API_KEY_PATH,
              appleApiKeyId: process.env.APPLE_API_KEY_ID,
              appleApiIssuer: process.env.APPLE_API_ISSUER,
          },
      }
    : {};

// Windows Authenticode signing, enabled only when the CI signing step has
// provisioned a signing hook (see the "Sign setup (Windows)" job step). Same
// gating rationale as macSign above: PR / fork builds with no secrets still
// produce an unsigned bundle instead of failing the whole job.
//
// WINDOWS_SIGN_HOOK_MODULE_PATH is @electron/windows-sign's own variable. It
// names a CommonJS module exporting `async (file) => void` that signs one PE
// file in place; windows-sign calls it for every .exe/.dll/.node in the
// packaged app, and the Squirrel maker calls it again for the installer. Which
// CA, which tool, and why are the CI step's business, not this file's.
const winSign = process.env.WINDOWS_SIGN_HOOK_MODULE_PATH
    ? { windowsSign: { hookModulePath: process.env.WINDOWS_SIGN_HOOK_MODULE_PATH } }
    : {};

// Architecture this `make` is producing for. Forge builds for the host arch
// unless told otherwise, so process.arch is right in CI; the argv sniffing
// keeps a manual `--arch` cross-build honest.
//
// Needed because electron-winstaller's setupExe is a fixed string with no
// {arch} template, while every maker's output for every platform lands in one
// flat GitHub Release namespace (see the upload step in
// .github/workflows/build-electron.yml). Without the suffix the x64 and arm64
// Windows jobs both emit `DarklySetup.exe`, and whichever finishes last
// clobbers the other: users on the losing arch download an installer whose
// payload their machine can't execute (ERROR_EXE_MACHINE_TYPE_MISMATCH). The
// mac/linux makers already put the arch in their filenames.
const targetArch = (() => {
    const flag = process.argv.indexOf('--arch');
    if (flag !== -1 && process.argv[flag + 1]) return process.argv[flag + 1];
    const inline = process.argv.find((a) => a.startsWith('--arch='));
    if (inline) return inline.slice('--arch='.length);
    return process.arch;
})();

/** @type {import('@electron-forge/shared-types').ForgeConfig} */
module.exports = {
    packagerConfig: {
        name: 'Darkly',
        executableName: 'darkly',
        // Offline builds (Flathub, distro packagers) point this at a directory
        // holding electron-v<version>-<platform>-<arch>.zip and the packager
        // unpacks that instead of asking @electron/get. A seeded @electron/get
        // cache is not enough: since v3 it refetches SHASUMS256.txt from
        // GitHub on every run, cache hit or not, so a sandbox with no network
        // fails at "Copying files" even with the zip already on disk.
        ...(process.env.DARKLY_ELECTRON_ZIP_DIR
            ? { electronZipDir: process.env.DARKLY_ELECTRON_ZIP_DIR }
            : {}),
        // Base path (no extension); packager appends .icns on macOS and .ico on
        // Windows. Linux packaging ignores this: the AppImage/deb makers below
        // take the .png explicitly.
        icon: path.resolve(__dirname, '..', 'packaging', 'icon'),
        asar: true,
        // Pack the frontend static dist alongside the packaged app at
        // resources/app/. main.ts reads from process.resourcesPath/app/.
        extraResource: [
            path.resolve(__dirname, 'resources/app'),
        ],
        ...macSign,
        ...winSign,
    },
    rebuildConfig: {},
    makers: [
        // macOS
        {
            name: '@electron-forge/maker-dmg',
            platforms: ['darwin'],
            config: {},
        },
        {
            name: '@electron-forge/maker-zip',
            platforms: ['darwin'],
            config: {},
        },

        // Linux
        {
            name: '@reforged/maker-appimage',
            platforms: ['linux'],
            config: {
                options: {
                    bin: 'darkly',
                    icon: path.resolve(__dirname, '..', 'packaging', 'icon.png'),
                    categories: app.categories,
                },
            },
        },
        {
            name: '@electron-forge/maker-deb',
            platforms: ['linux'],
            config: {
                options: {
                    bin: 'darkly',
                    icon: path.resolve(__dirname, '..', 'packaging', 'icon.png'),
                    maintainer: 'Darkly <info@darkly.art>',
                    homepage: 'https://darkly.art',
                    section: 'graphics',
                    categories: app.categories,
                    description: app.summary,
                },
            },
        },

        // Windows
        {
            name: '@electron-forge/maker-squirrel',
            platforms: ['win32'],
            config: {
                name: 'darkly',
                setupExe: `DarklySetup-${targetArch}.exe`,
                // Signs the generated Setup.exe. This is separate from the
                // packagerConfig.windowsSign above, which signs what goes
                // INSIDE it (darkly.exe, Update.exe, the Electron DLLs). Both
                // are needed: Setup.exe is what SmartScreen judges at download
                // time, while the inner binaries are what Smart App Control,
                // AppLocker/WDAC publisher rules and most EDR heuristics judge
                // at launch time, and Squirrel deploys them unchanged on every
                // auto-update.
                ...winSign,
            },
        },
    ],
    plugins: [],
};
