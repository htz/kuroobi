import { useSyncExternalStore } from 'react';
import en from '../locales/en.yaml';
import ja from '../locales/ja.yaml';


export const LANGS = ['en', 'ja'] as const;
export type Lang = (typeof LANGS)[number];
export type LangPref = 'auto' | Lang;

type Tree = { [k: string]: string | Tree };

function flatten(tree: Tree, prefix = '', out: Record<string, string> = {}) {
  for (const [k, v] of Object.entries(tree)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (typeof v === 'string') out[key] = v;
    else flatten(v, key, out);
  }
  return out;
}

const TABLES: Record<Lang, Record<string, string>> = {
  en: flatten(en as Tree),
  ja: flatten(ja as Tree),
};

function pick(tags: readonly string[]): Lang | '' {
  for (const tag of tags) {
    const base = tag.toLowerCase().split('-')[0];
    if ((LANGS as readonly string[]).includes(base)) return base as Lang;
  }
  return '';
}

export function systemLang(osTag = ''): Lang {
  return pick([osTag]) || pick(navigator.languages?.length
    ? navigator.languages : [navigator.language]) || 'en';
}

export const resolveLang = (pref: LangPref, osTag = ''): Lang =>
  (pref === 'auto' ? systemLang(osTag) : pref);

let current: Lang = 'en';
const listeners = new Set<() => void>();

export function setLang(lang: Lang) {
  if (lang === current) return;
  current = lang;
  document.documentElement.lang = lang;
  for (const fn of listeners) fn();
}

export const getLang = (): Lang => current;

export function useLang(): Lang {
  return useSyncExternalStore(
    (fn) => { listeners.add(fn); return () => listeners.delete(fn); },
    getLang,
  );
}

export type Params = Record<string, string | number>;

export function t(key: string, params?: Params): string {
  const s = TABLES[current][key] ?? TABLES.en[key] ?? key;
  if (!params) return s;
  return s.replace(/\{(\w+)\}/g, (m, name: string) =>
    name in params ? String(params[name]) : m);
}

export function backendStrings(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(TABLES[current])) {
    if (k.startsWith('backend.')) out[k] = v;
  }
  for (const [k, v] of Object.entries(TABLES.en)) {
    if (k.startsWith('backend.') && !(k in out)) out[k] = v;
  }
  return out;
}

export function tErr(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  if (!/^err\.[a-z0-9_.]+/.test(raw)) return raw;
  const [key, ...rest] = raw.split('|');
  const params: Params = {};
  for (const pair of rest) {
    const at = pair.indexOf('=');
    if (at > 0) params[pair.slice(0, at)] = pair.slice(at + 1);
  }
  return t(key, params);
}
