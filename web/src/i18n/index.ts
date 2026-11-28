import en from './en.json';
import ja from './ja.json';

export const LANGS = ['ja', 'en'] as const;
export type Lang = (typeof LANGS)[number];

const DICT: Record<Lang, unknown> = { ja, en };

const lookup = (lang: Lang, key: string): string | undefined => {
  let v: unknown = DICT[lang];
  for (const part of key.split('.')) v = (v as Record<string, unknown> | undefined)?.[part];
  return typeof v === 'string' ? v : undefined;
};

/** The text for `key`, English when a language lacks it; `{name}` takes `vars.name`. */
export function t(lang: Lang, key: string, vars: Record<string, string | number> = {}): string {
  const s = lookup(lang, key) ?? lookup('en', key) ?? key;
  return s.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? String(vars[k]) : m));
}

/** A site path (`/play/`) in `lang`: Japanese at the root, English under /en/. */
export const href = (lang: Lang, path: string): string => (lang === 'ja' ? path : '/en' + path);

/** The language a URL path is in, and the path with that prefix taken off. */
export function splitPath(pathname: string): { lang: Lang; path: string } {
  const m = pathname.match(/^\/en(\/.*)?$/);
  return m ? { lang: 'en', path: m[1] ?? '/' } : { lang: 'ja', path: pathname };
}
