// Electron Forge configuration.
//
// Plain CommonJS (not TypeScript) so the Forge CLI can load it without an
// extra ts-node loader.

const path = require('path');

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
// provisioned credentials (see the "Sign setup (Windows)" job step). Same
// gating rationale as macSign above: PR / fork builds with no secrets still
// produce an unsigned bundle instead of failing the whole job.
//
// Signing is necessary but not sufficient for a clean install. SmartScreen
// weighs two signals, the file hash's reputation and the publisher
// certificate's, and a brand-new binary has neither, so the first downloads
// still get "unrecognized app". What signing buys is that reputation ACCRUES:
// a consistent publisher identity carries trust forward to the next release,
// whereas unsigned files start from zero every single time and never stop
// warning. EV certificates used to bypass the prompt outright; Microsoft
// removed that in 2024, so there is no longer any certificate you can buy
// that skips the ramp. A self-signed certificate is worth exactly nothing
// here: SmartScreen treats it identically to no signature at all.
//
// Deliberately provider-agnostic. Everything specific to the CA lives in the
// CI step, which hands this block raw signtool arguments; today that is
// SSL.com eSigner CKA, which loads the cloud-held certificate into the
// Windows certificate store so signtool can select it by thumbprint:
//
//   DARKLY_WIN_SIGN_PARAMS   e.g. "/sha1 <thumbprint>"
//   DARKLY_WIN_TIMESTAMP_URL e.g. "http://ts.ssl.com"
//   SIGNTOOL_PATH            Windows SDK signtool.exe
//
// @electron/windows-sign also reads WINDOWS_CERTIFICATE_FILE and
// WINDOWS_CERTIFICATE_PASSWORD from the environment on its own, which is the
// escape hatch for signing locally against a throwaway self-signed .pfx to
// prove the pipeline works. Since the 2023 CA/Browser Forum rules put every
// issued code-signing key on hardware, a real certificate will never arrive
// as a .pfx you can commit to a secret.
//
// hashes is pinned to sha256 deliberately. windows-sign's default when the
// option is omitted is ['sha1', 'sha256']: it signs twice and appends, and the
// SHA-1 pass fails against any certificate issued today. SHA-1 has been
// untrusted on Windows since 2016 anyway, so there is nothing to lose.
const winSign = process.env.DARKLY_WIN_SIGN === '1'
    ? {
          windowsSign: {
              hashes: ['sha256'],
              // RFC 3161 timestamp, so signatures stay valid past certificate
              // expiry. Non-negotiable now that CA/Browser Forum ballot CSC-31
              // caps certificate lifetime at 460 days: without a countersigned
              // timestamp every release would go untrusted within ~15 months.
              timestampServer:
                  process.env.DARKLY_WIN_TIMESTAMP_URL
                  || 'http://ts.ssl.com',
              // signtool /d and /du: shown in the UAC elevation dialog.
              description: 'Darkly',
              website: 'https://darkly.art',
              // Windows SDK signtool. Without this, windows-sign falls back to
              // a vendored copy too old to drive a cloud-held key.
              ...(process.env.SIGNTOOL_PATH
                  ? { signToolPath: process.env.SIGNTOOL_PATH }
                  : {}),
              ...(process.env.DARKLY_WIN_SIGN_PARAMS
                  ? { signWithParams: process.env.DARKLY_WIN_SIGN_PARAMS }
                  : {}),
          },
      }
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
                    categories: ['Graphics', '2DGraphics'],
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
                    categories: ['Graphics'],
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
