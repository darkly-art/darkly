// The one naming rule for the release artifacts the makers produce:
//
//   Darkly-<version>-<os>-<arch>.<ext>
//
// Each maker names its output its own way: Squirrel's setupExe is a fixed
// string with no arch, maker-zip's name is not configurable at all, and
// maker-dmg and the AppImage maker each have their own template. All of them
// land in one flat GitHub Release namespace that the website, the itch.io
// push and humans read by filename, so the make's postMake hook renames them
// here, in one place, instead of each maker being coaxed separately. The arch
// in the name is also what stops the x64 and arm64 jobs of one platform
// clobbering each other's upload (see the upload step in darkly-deploy's
// build-electron.yml).
//
// Debian packages keep dpkg's own `name_version_arch.deb` form, which is a
// standard in its own right, and Squirrel's RELEASES/nupkg feed is left alone:
// it is not uploaded, and RELEASES indexes the nupkg by name.

const path = require('path');

const OS = { darwin: 'macos', win32: 'windows', linux: 'linux' };
const RENAMED = new Set(['.exe', '.dmg', '.zip', '.AppImage']);

/**
 * The standardized filename for a maker output, or null for an output that
 * keeps the name its maker gave it.
 *
 * @param {string} file path or name of the maker output
 * @param {{ version: string, platform: string, arch: string }} make
 *   the package.json version and Forge's platform/arch of the make
 * @returns {string | null}
 */
function artifactName(file, { version, platform, arch }) {
    const ext = path.extname(file);
    if (!RENAMED.has(ext)) return null;
    const os = OS[platform];
    if (!os) throw new Error(`no release artifact OS name for platform ${platform}`);
    return `Darkly-${version}-${os}-${arch}${ext}`;
}

module.exports = { artifactName };
