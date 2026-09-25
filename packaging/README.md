# Packaging resources

Desktop-integration metadata and artwork shared by every channel Darkly ships
through. One home each, so a description or an icon is never restated per
packaging target.

| File | Consumed by |
| --- | --- |
| `art.darkly.Darkly.desktop` | Flathub manifest; the deb and AppImage makers |
| `art.darkly.Darkly.metainfo.xml` | AppStream, so GNOME Software, KDE Discover and Bazaar |
| `icon.png` | Flathub (installed as `art.darkly.Darkly.png`), deb, AppImage |
| `icon.icns` | macOS `.dmg` and `.zip` |
| `icon.ico` | Windows installer |

The icons keep the plain `icon` stem rather than the app ID. Electron's packager
derives the platform icon by stripping the extension and appending its own, and
`path.extname('art.darkly.Darkly')` is `.Darkly`, so an app-ID stem makes it look
for `art.darkly.icns`, fail to find it, warn rather than error, and ship macOS
and Windows with no icon at all. Consumers that need the app-ID filename rename
on install, which is one line in the Flathub manifest.

## What does not belong here

Anything specific to a single packaging target. The Flathub launcher, which
wraps the binary in `zypak-wrapper`, is the worked example: zypak exists only
inside `org.electronjs.Electron2.BaseApp`, so the launcher is meaningless to
every other channel and belongs with the Flathub manifest rather than here,
whether it is carried as its own file or declared inline in that manifest. The
same rule applies to any future snapcraft or winget file: shared facts here,
per-channel recipes with their channel.

## Flathub

Darkly is published to Flathub as `art.darkly.Darkly`. Flathub builds from
source on its own infrastructure with no network access, so every dependency is
vendored as a pinned source list rather than fetched at build time, and the
manifest lives in the `flathub/art.darkly.Darkly` repository rather than here.

The summary, description and desktop-entry fields in the two text files are
generated from `crates/darkly/product.yaml`; `cargo sync-docs` refills them.
The metainfo's `<releases>` block is generated from the `v*` tags, never edited
by hand. The committed copy is refreshed after each release
([`docs/versioning.md`](../docs/versioning.md)), so it lags a tag until that
commit lands; a channel therefore refills it at build time from the tags
reachable from its checkout, and installs the result:

```bash
scripts/metainfo-releases.sh packaging/art.darkly.Darkly.metainfo.xml > art.darkly.Darkly.metainfo.xml
```

The tags are the release record, so a channel that builds from a clone must
fetch them. With no tags at all (a tarball) the script keeps the committed block
as it is.

Validate changes to the two text files in this directory the way CI does:

```bash
scripts/metainfo-releases.sh packaging/art.darkly.Darkly.metainfo.xml > /tmp/art.darkly.Darkly.metainfo.xml
appstreamcli validate --no-net /tmp/art.darkly.Darkly.metainfo.xml
desktop-file-validate packaging/art.darkly.Darkly.desktop
```

The full manifest build and lint recipe is documented alongside the manifest in
the Flathub repository, since it needs the multi-gigabyte Freedesktop SDK and
the Electron base app.
