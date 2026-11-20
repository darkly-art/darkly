/*
 * Build the search index.
 *
 * Pagefind reads the built HTML and writes `<outDir>/pagefind/`: the index plus
 * the JS API a page loads at runtime to query it.
 *
 * An integration rather than a build-script step, so that every path into a
 * build gets it: `astro build` and `npm run build` alike. The failure mode of
 * forgetting is a site whose search box finds nothing, which is easy to ship
 * and hard to notice.
 *
 * Only elements a site marks `data-pagefind-body` are indexed, so nothing
 * outside them, a sidebar or a header, ends up in an excerpt.
 */
import { fileURLToPath } from 'node:url';

export default function pagefind() {
    return {
        name: 'darkly:pagefind',
        hooks: {
            'astro:build:done': async ({ dir, logger }) => {
                const site = fileURLToPath(dir);
                const started = performance.now();

                // Imported here, not at module scope: `astro dev` loads this
                // file too, and the dev server should not pay for a dependency
                // it will never call.
                const pagefind = await import('pagefind');

                const { errors, index } = await pagefind.createIndex();
                if (errors?.length) throw new Error(errors.join('\n'));

                const added = await index.addDirectory({ path: site });
                if (added.errors?.length) throw new Error(added.errors.join('\n'));

                const written = await index.writeFiles({ outputPath: `${site}/pagefind` });
                if (written.errors?.length) throw new Error(written.errors.join('\n'));

                await pagefind.close();

                const ms = Math.round(performance.now() - started);
                // `page_count` is what addDirectory *scanned*, not what landed
                // in the index. Pages without `data-pagefind-body` are counted
                // here and then dropped. The real figure is `page_count` in
                // pagefind/pagefind-entry.json.
                logger.info(`Scanned ${added.page_count} page(s) for search in ${ms}ms`);
            },
        },
    };
}
