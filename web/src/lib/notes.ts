import { getCollection, type CollectionEntry } from 'astro:content';
import type { Lang } from '../i18n';

export type Note = CollectionEntry<'notes'>;

/** The article's name in URLs: its file name, the same in both languages. */
export const slugOf = (n: Note): string => n.id.split('/').slice(1).join('/');

/** One language's articles, newest first. */
export async function notesIn(lang: Lang): Promise<Note[]> {
  const all = await getCollection('notes', (n) => n.id.startsWith(lang + '/'));
  return all.sort((a, b) => b.data.date.getTime() - a.data.date.getTime());
}

/** Minutes to read: about 600 characters a minute in Japanese, 220 words in English. */
export function readingMinutes(n: Note, lang: Lang): number {
  const body = n.body ?? '';
  const units = lang === 'ja' ? body.replace(/\s/g, '').length / 600 : body.split(/\s+/).length / 220;
  return Math.max(1, Math.round(units));
}

export const dateText = (d: Date, lang: Lang): string =>
  d.toLocaleDateString(lang === 'ja' ? 'ja-JP' : 'en-GB', { year: 'numeric', month: 'long', day: 'numeric', timeZone: 'UTC' });
