import { useCallback, useEffect, useState } from 'react';
import { api } from './api';
import { backendStrings, resolveLang, setLang, type LangPref } from './i18n';


export type Theme = 'os' | 'dark' | 'light';
export type Facing = 'black' | 'white' | 'auto';

export type Tatami = 0 | 1 | 2 | 3;
export type Decimals = 0 | 1 | 2;

export interface Prefs {
  theme: Theme;
  tatami: Tatami;
  decimals: Decimals;
  coords: boolean;
  grain: boolean;
  flipMs: 0 | 120 | 240;
  facing: Facing;
  clockSecs: number;
  lang: LangPref;
}

const DEFAULTS: Prefs = {
  theme: 'os', tatami: 0, decimals: 1,
  coords: true, grain: true, flipMs: 120, facing: 'black',
  clockSecs: 0, lang: 'auto',
};

export const TATAMI: { labelKey: string; board: string; dark: string; line: string; grain: string }[] = [
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

  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key !== null && e.key !== KEY) return;
      setPrefs(load());
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);

  const [forced, setForced] = useState<Theme | ''>('');
  useEffect(() => {
    void api.themeOverride()
      .then((t) => { if (t === 'light' || t === 'dark') setForced(t); })
      .catch(() => { /* simply inert outside Tauri or on old binaries */ });
  }, []);

  const [forcedLang, setForcedLang] = useState<LangPref | ''>('');
  useEffect(() => {
    void api.langOverride()
      .then((l) => { if (l === 'en' || l === 'ja') setForcedLang(l); })
      .catch(() => { /* inert outside Tauri or on older binaries */ });
  }, []);

  const [osLang, setOsLang] = useState('');
  useEffect(() => {
    void api.systemLang()
      .then(setOsLang)
      .catch(() => { /* falls back to the navigator values */ });
  }, []);

  useEffect(() => {
    setLang(resolveLang(forcedLang || prefs.lang, osLang));
    void api.setBackendStrings(backendStrings()).catch(() => { /* older binaries */ });
  }, [prefs.lang, forcedLang, osLang]);

  useEffect(() => {
    const el = document.documentElement;
    const t = forced || prefs.theme;
    if (t === 'os') el.removeAttribute('data-theme');
    else el.setAttribute('data-theme', t);
  }, [prefs.theme, forced]);

  useEffect(() => {
    document.documentElement.style.setProperty('--flip-dur', prefs.flipMs + 'ms');
  }, [prefs.flipMs]);

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

export const flipped = (facing: Facing, myColor: 'black' | 'white' | ''): boolean =>
  facing === 'white' || (facing === 'auto' && myColor === 'white');

