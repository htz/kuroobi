import { useSyncExternalStore } from 'react';
import en from '../locales/en.yaml';
import ja from '../locales/ja.yaml';

/* Translation lookup.
 *
 * Every user-visible string lives in `locales/<lang>.yaml`; no
 * implementation file carries display text. English is the fallback
 * for every language, so a key missing from ja still renders.
 *
 * `t()` is a plain function, not a hook, because tables and helpers
 * outside React need it too. Components re-render on a language
 * change through `useLang()`, which every screen root subscribes to. */

export const LANGS = ['en', 'ja'] as const;
export type Lang = (typeof LANGS)[number];
/** `auto` follows the machine's language (the default). */
export type LangPref = 'auto' | Lang;

type Tree = { [k: string]: string | Tree };

/** Dotted keys, so YAML can nest while lookup stays a map hit. */
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

/** Pick a supported language from BCP 47 tags, most preferred first. */
function pick(tags: readonly string[]): Lang | '' {
  for (const tag of tags) {
    const base = tag.toLowerCase().split('-')[0];
    if ((LANGS as readonly string[]).includes(base)) return base as Lang;
  }
  return '';
}

/** The machine's language, used when the preference is `auto`.
 *
 *  `osTag` comes from the backend and wins: `navigator.language`
 *  follows the app bundle's localizations, not the system, so an
 *  unlocalized build reports en-US on a Japanese machine. The
 *  navigator values remain the fallback for when the backend cannot
 *  answer (outside Tauri, or an older binary). */
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

/** Subscribe a component tree to language changes. */
export function useLang(): Lang {
  return useSyncExternalStore(
    (fn) => { listeners.add(fn); return () => listeners.delete(fn); },
    getLang,
  );
}

/** Values interpolated into `{name}` placeholders. */
export type Params = Record<string, string | number>;

/** Look up `key`, falling back to English and then to the key itself.
 *  A visible key is the loudest possible sign of a missing entry. */
export function t(key: string, params?: Params): string {
  const s = TABLES[current][key] ?? TABLES.en[key] ?? key;
  if (!params) return s;
  return s.replace(/\{(\w+)\}/g, (m, name: string) =>
    name in params ? String(params[name]) : m);
}

/** Keys the backend needs (OS notifications, native file dialogs).
 *  It cannot read the YAML itself, so the current language's subset
 *  is pushed to it at startup and on every change. */
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

/** Translate an error raised by a backend command.
 *
 * Rust commands return an i18n key instead of a message, optionally
 * carrying values: `err.resource_missing|what=NNUE|path=/x`. Anything
 * that does not look like a key (a panic, an IPC failure) is shown
 * verbatim — an unreadable message beats a swallowed one. */
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
