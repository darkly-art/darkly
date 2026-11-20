#!/usr/bin/env node
/*
 * src/shared/ builds from a copy of itself alone.
 *
 * Other sites receive this directory as source and compile it with their own
 * toolchain, and nothing else of this project. A shared file that imports
 * from elsewhere in docs-site/, or from a package only docs-site/ installs,
 * still builds here, so only this check catches it before a release does.
 *
 * Every file must be one this check reads or a known binary. In the files it
 * reads, a relative specifier (import, dynamic import, CSS @import or url())
 * must name an existing file inside src/shared/; a root-absolute path names a
 * file the consuming site would have to serve; a bare specifier must be one of
 * PACKAGES or a `node:` builtin, the frameworks the shared files are written
 * against; and `import.meta.glob` is refused, since its pattern can reach
 * anywhere.
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SHARED = fileURLToPath(new URL('../src/shared/', import.meta.url));
const PACKAGES = ['astro', 'svelte', 'pagefind'];
const READ = /\.(astro|svelte|ts|js|mjs|css)$/;
const BINARY = /\.(png|woff2)$/;
const SPECIFIERS = [
    /\bfrom\s*['"]([^'"]+)['"]/g,
    /\bimport\s*\(\s*['"]([^'"]+)['"]/g,
    /\bimport\s+['"]([^'"]+)['"]/g,
    /@import\s+['"]([^'"]+)['"]/g,
    /\burl\(\s*['"]?([^'")]+?)['"]?\s*\)/g,
];

const problems = [];
for (const rel of fs.readdirSync(SHARED, { recursive: true })) {
    const file = path.join(SHARED, rel);
    if (fs.statSync(file).isDirectory() || BINARY.test(file)) continue;
    if (!READ.test(file)) {
        problems.push(`${rel}: not a file type this check reads`);
        continue;
    }
    const text = fs.readFileSync(file, 'utf8');
    if (text.includes('import.meta.glob')) problems.push(`${rel}: import.meta.glob can reach outside src/shared/`);
    for (const pattern of SPECIFIERS) {
        for (const [, spec] of text.matchAll(pattern)) {
            if (/^(data:|https?:|#)/.test(spec)) continue;
            if (spec.startsWith('/')) {
                problems.push(`${rel}: '${spec}' is root-absolute, a file the consuming site must serve`);
            } else if (spec.startsWith('.')) {
                const target = path.resolve(path.dirname(file), spec.split(/[?#]/)[0]);
                if (path.relative(SHARED, target).startsWith('..')) problems.push(`${rel}: '${spec}' leaves src/shared/`);
                else if (!fs.existsSync(target)) problems.push(`${rel}: '${spec}' does not exist`);
            } else if (!spec.startsWith('node:') && !PACKAGES.includes(spec.split('/')[0])) {
                problems.push(`${rel}: '${spec}' is not one of ${PACKAGES.join(', ')}`);
            }
        }
    }
}

if (problems.length) {
    console.error(problems.join('\n'));
    process.exit(1);
}
console.log('src/shared/ is self-contained');
