import { useCallback, useEffect, useState } from 'react';

/* 見え方の好み。エンジンの動きには一切関わらないので、バックエンドには
 * 持たせず localStorage に置く (機械ごとの設定でよく、保存の往復も要らない)。
 *
 * 既定はすべて「いまの見え方」に揃えてある — 設定を足したせいで、何もして
 * いない人の画面が変わることがないように。
 */

export type Theme = 'os' | 'dark' | 'light';
/** 盤の向き。`auto` は自分が持っている色を下にする (対局のときだけ効く)。 */
export type Facing = 'black' | 'white' | 'auto';

export interface Prefs {
  theme: Theme;
  /** 盤の縁の a〜h / 1〜8。 */
  coords: boolean;
  /** 畳の藺草の目。薄いので普段は気にならないが、消したい人もいる。 */
  grain: boolean;
  /** 石が返るときの動き (ミリ秒)。0 で動かさない。 */
  flipMs: 0 | 120 | 240;
  facing: Facing;
}

const DEFAULTS: Prefs = {
  theme: 'os', coords: true, grain: true, flipMs: 120, facing: 'black',
};

const KEY = 'kuroobi.prefs';

function load(): Prefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return DEFAULTS;
    // 知らないキーは捨て、足りないキーは既定で埋める。設定を足したときに
    // 古い保存が読めなくなると、黙って全部が既定へ戻る
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
      try { localStorage.setItem(KEY, JSON.stringify(next)); } catch { /* 保存できなくても続ける */ }
      return next;
    });
  }, []);

  /* 設定は別の窓で触るので、こちらは書き換えを聞いて追いかける。
   * 同じ生成元の document どうしなら storage が飛ぶ (自分が書いたときは
   * 飛ばないので、書いた側の setPrefs と二重にはならない)。 */
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key !== null && e.key !== KEY) return;
      setPrefs(load());
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);

  // テーマは :root の属性で切り替える。`os` のときは属性を外して
  // prefers-color-scheme に任せる (tokens.css がその形で書いてある)
  useEffect(() => {
    const el = document.documentElement;
    if (prefs.theme === 'os') el.removeAttribute('data-theme');
    else el.setAttribute('data-theme', prefs.theme);
  }, [prefs.theme]);

  // 石返しの長さは CSS 変数。base.css の .k-flip がこれを見る
  useEffect(() => {
    document.documentElement.style.setProperty('--flip-dur', prefs.flipMs + 'ms');
  }, [prefs.flipMs]);

  return { prefs, set };
}

/** 盤を回すか。`auto` は自分の色が白のときだけ回す (自分を下に置く)。 */
export const flipped = (facing: Facing, myColor: 'black' | 'white' | ''): boolean =>
  facing === 'white' || (facing === 'auto' && myColor === 'white');

/** 回した盤のマス番号。a1 が右上に来る (file も rank も裏返す)。 */
export const viewSq = (sq: number, flip: boolean): number => (flip ? 63 - sq : sq);
