import { defineCollection } from 'astro:content';
import { glob } from 'astro/loaders';
import { z } from 'astro/zod';

export const TAGS = ['web', 'eval', 'search', 'board', 'book'] as const;

/** One file per article and language: `notes/ja/<slug>.mdx` and `notes/en/<slug>.mdx`. */
const notes = defineCollection({
  loader: glob({ base: './src/content/notes', pattern: '{ja,en}/*.{md,mdx}' }),
  schema: z.object({
    title: z.string(),
    summary: z.string(),
    date: z.coerce.date(),
    tags: z.array(z.enum(TAGS)).min(1),
  }),
});

export const collections = { notes };
