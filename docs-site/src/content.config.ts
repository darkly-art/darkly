import { defineCollection } from 'astro:content';
import { glob } from 'astro/loaders';
import { z } from 'astro/zod';

/*
 * Every page here is written by scripts/gen-docs.mjs: reference tables from the
 * registries, prose copied out of the manual.
 *
 * Strict, so a misspelled frontmatter key fails the build rather than being
 * dropped without a word.
 */
const docs = defineCollection({
    loader: glob({ pattern: '**/*.{md,mdx}', base: './src/content/docs' }),
    schema: z
        .object({
            title: z.string(),
            description: z.string().optional(),
            // Position within a sidebar group. Reference pages carry one so the
            // catalog order in gen-docs.mjs survives; prose pages sort by title.
            order: z.number().optional(),
        })
        .strict(),
});

export const collections = { docs };
