/**
 * Release artifacts share one name scheme, Darkly-<version>-<os>-<arch>.<ext>,
 * applied after the make. The website classifies downloads by filename and
 * the itch.io push looks each bundle up by it, so the scheme is a contract.
 */
import { describe, it, expect } from 'vitest';
import { artifactName } from '../../artifact-name';

const v = '0.11.0';

describe('release artifact naming', () => {
    it.each([
        ['DarklySetup.exe', 'win32', 'x64', 'Darkly-0.11.0-windows-x64.exe'],
        ['DarklySetup.exe', 'win32', 'arm64', 'Darkly-0.11.0-windows-arm64.exe'],
        ['Darkly-0.11.0-arm64.dmg', 'darwin', 'arm64', 'Darkly-0.11.0-macos-arm64.dmg'],
        ['Darkly-darwin-x64-0.11.0.zip', 'darwin', 'x64', 'Darkly-0.11.0-macos-x64.zip'],
        ['Darkly-0.11.0-x64.AppImage', 'linux', 'x64', 'Darkly-0.11.0-linux-x64.AppImage'],
    ])('renames %s for %s/%s to %s', (file, platform, arch, expected) => {
        expect(artifactName(`/out/make/${file}`, { version: v, platform, arch })).toBe(expected);
    });

    it('keeps the arch out of the clobber zone: the two Windows installers differ by name', () => {
        const make = (arch: string) => artifactName('DarklySetup.exe', { version: v, platform: 'win32', arch });
        expect(make('x64')).not.toBe(make('arm64'));
    });

    it.each([
        'darkly-desktop_0.11.0_amd64.deb',
        'RELEASES',
        'darkly-0.11.0-full.nupkg',
    ])('leaves %s to its maker', (file) => {
        expect(artifactName(`/out/make/${file}`, { version: v, platform: 'linux', arch: 'x64' })).toBeNull();
    });

    it('refuses a platform it has no OS name for', () => {
        expect(() => artifactName('a.exe', { version: v, platform: 'freebsd', arch: 'x64' })).toThrow(/freebsd/);
    });
});
