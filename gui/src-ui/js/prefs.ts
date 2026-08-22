import { useCallback, useEffect, useState } from 'react';
import { api } from './api';
import { backendStrings, resolveLang, setLang, type LangPref } from './i18n';

/* Display preferences. They never affect the engine, so they live in
 * localStorage (per-machine, no round-trips). Every default matches
 * the current look — adding a setting must not change anyone's screen. */

export type Theme = 'os' | 'dark' | 'light';
/** Board facing; `auto` puts our color at the bottom (games only). */
export type Facing = 'black' | 'white' | 'auto';

/** Board mat color; the Display tab shows four swatches. */
export type Tatami = 0 | 1 | 2 | 3;
/** Eval decimal places (0 / 1 / 2 per the Display tab). */
export type Decimals = 0 | 1 | 2;

export interface Prefs {
  theme: Theme;
  /** Mat color (0 = default). */
  tatami: Tatami;
  /** Eval decimal places. */
  decimals: Decimals;
  /** Board edge coordinates a-h / 1-8. */
  coords: boolean;
  /** The mat's grain texture; subtle, but some want it off. */
  grain: boolean;
  /** Disc-flip animation (ms); 0 disables. */
  flipMs: 0 | 120 | 240;
  facing: Facing;
  /** Local game clock (seconds); 0 = none. Applies from the next new game. */
  clockSecs: number;
  /** UI language; `auto` follows the machine's language. */
  lang: LangPref;
}

const DEFAULTS: Prefs = {
  theme: 'os', tatami: 0, decimals: 1,
  coords: true, grain: true, flipMs: 120, facing: 'black',
  clockSecs: 0, lang: 'auto',
};

/* Mat colors. The board's four tokens swap as a set — changing only
 * the ground leaves edges/lines/grain behind and muddies the board. */
export const TATAMI: { labelKey: string; board: string; dark: string; line: string; grain: string }[] = [
  // Swatch colors measured from the design; edges/lines/grain derived.
  { labelKey: 'settings.tatami.default', board: '#77914e', dark: '#3f4f2c', line: '#3d5226', grain: '#33421d' },
  { labelKey: 'settings.tatami.straw', board: '#8a8f5c', dark: '#474a2f', line: '#464a28', grain: '#3b3f1f' },
  { labelKey: 'settings.tatami.moss', board: '#6f7f6a', dark: '#3a4238', line: '#374033', grain: '#2f382c' },
  { labelKey: 'settings.tatami.forest', board: '#3f4f2c', dark: '#232c18', line: '#212b14', grain: '#1b230f' },
];

const KEY = 'kuroobi.prefs';

function load(): Prefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return DEFAULTS;
    // Unknown keys dropped, missing keys defaulted — an unreadable old
    // save must not silently reset everything.
    const got = JSON.parse(raw) as Partial<Prefs>;
    return { ...DEFAULTS, ...got };
  } catch {
    return DEFAULTS;
  }
}

export function usePrefs() {
  const [prefs, setPrefs] = useState<Prefs>(load);

  const set = useCallback(<K extends keyof Prefs>(k: K, v: Prefs[K]) => {
    setPrefs((p) => {
      const next = { ...p, [k]: v };
      try { localStorage.setItem(KEY, JSON.stringify(next)); } catch { /* keep going even if the save fails */ }
      return next;
    });
  }, []);

  /* Track external writes via the storage event (it never fires for
   * our own writes, so no doubling with setPrefs). */
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key !== null && e.key !== KEY) return;
      setPrefs(load());
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);

  /* Screenshot pin (`KUROOBI_THEME=light`); not persisted, a plain
     launch restores the preference. */
  const [forced, setForced] = useState<Theme | ''>('');
  useEffect(() => {
    void api.themeOverride()
      .then((t) => { if (t === 'light' || t === 'dark') setForced(t); })
      .catch(() => { /* simply inert outside Tauri or on old binaries */ });
  }, []);

  /* Screenshot pin (`KUROOBI_LANG=en`); not persisted, like the theme
     override above. */
  const [forcedLang, setForcedLang] = useState<LangPref | ''>('');
  useEffect(() => {
    void api.langOverride()
      .then((l) => { if (l === 'en' || l === 'ja') setForcedLang(l); })
      .catch(() => { /* inert outside Tauri or on older binaries */ });
  }, []);

  /* Apply the language before anything renders text, and hand the
     backend its subset (it owns OS notifications and native dialogs,
     which no frontend translation can reach). */
  useEffect(() => {
    setLang(resolveLang(forcedLang || prefs.lang));
    void api.setBackendStrings(backendStrings()).catch(() => { /* older binaries */ });
  }, [prefs.lang, forcedLang]);

  // Theme switches via a :root attribute; `os` removes it and defers
  // to prefers-color-scheme (tokens.css is written that way).
  useEffect(() => {
    const el = document.documentElement;
    const t = forced || prefs.theme;
    if (t === 'os') el.removeAttribute('data-theme');
    else el.setAttribute('data-theme', t);
  }, [prefs.theme, forced]);

  // Flip duration is a CSS variable read by base.css's .k-flip.
  useEffect(() => {
    document.documentElement.style.setProperty('--flip-dur', prefs.flipMs + 'ms');
  }, [prefs.flipMs]);

  /* Mat color overrides the tokens, surviving theme switches. Default
   * (0) writes nothing — light mode has its own green and an override
   * would ruin it. */
  useEffect(() => {
    const el = document.documentElement;
    const keys = ['--board', '--board-dark', '--line', '--grain'];
    if (prefs.tatami === 0) { for (const k of keys) el.style.removeProperty(k); return; }
    const t = TATAMI[prefs.tatami];
    el.style.setProperty('--board', t.board);
    el.style.setProperty('--board-dark', t.dark);
    el.style.setProperty('--line', t.line);
    el.style.setProperty('--grain', t.grain);
  }, [prefs.tatami]);

  return { prefs, set };
}

/** Whether to flip the board; `auto` flips only when we play White. */
export const flipped = (facing: Facing, myColor: 'black' | 'white' | ''): boolean =>
  facing === 'white' || (facing === 'auto' && myColor === 'white');

