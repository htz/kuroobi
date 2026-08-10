import { useCallback, useEffect, useState } from 'react';
import { api } from './api';

/* 見え方の好み。エンジンの動きには一切関わらないので、バックエンドには
 * 持たせず localStorage に置く (機械ごとの設定でよく、保存の往復も要らない)。
 *
 * 既定はすべて「いまの見え方」に揃えてある — 設定を足したせいで、何もして
 * いない人の画面が変わることがないように。
 */

export type Theme = 'os' | 'dark' | 'light';
/** 盤の向き。`auto` は自分が持っている色を下にする (対局のときだけ効く)。 */
export type Facing = 'black' | 'white' | 'auto';

/** 畳の色。設計の 表示 タブが 4 色の見本を並べている。 */
export type Tatami = 0 | 1 | 2 | 3;
/** 評価値の小数桁。設計の 表示 タブが 0 / 1 / 2 を出している。 */
export type Decimals = 0 | 1 | 2;

export interface Prefs {
  theme: Theme;
  /** 畳の色 (0 = 標準)。 */
  tatami: Tatami;
  /** 評価値の小数桁。 */
  decimals: Decimals;
  /** 盤の縁の a〜h / 1〜8。 */
  coords: boolean;
  /** 畳の藺草の目。薄いので普段は気にならないが、消したい人もいる。 */
  grain: boolean;
  /** 石が返るときの動き (ミリ秒)。0 で動かさない。 */
  flipMs: 0 | 120 | 240;
  facing: Facing;
  /** ローカル対局の持ち時間 (秒)。0 で時計なし。**次の新規対局から効く** */
  clockSecs: number;
}

const DEFAULTS: Prefs = {
  theme: 'os', tatami: 0, decimals: 1,
  coords: true, grain: true, flipMs: 120, facing: 'black',
  clockSecs: 0,
};

/* 畳の色。**盤の 4 つのトークンを組で差し替える** — 地だけ変えると
 * 縁と罫と藺草の目が取り残されて、盤が濁って見える。
 * 色は設計の 表示 タブの見本から取った。 */
export const TATAMI: { label: string; board: string; dark: string; line: string; grain: string }[] = [
  // 見本の色は設計の実測値。縁・罫・藺草の目はそれに合わせて落とした
  { label: '標準', board: '#77914e', dark: '#3f4f2c', line: '#3d5226', grain: '#33421d' },
  { label: '枯草', board: '#8a8f5c', dark: '#474a2f', line: '#464a28', grain: '#3b3f1f' },
  { label: '苔', board: '#6f7f6a', dark: '#3a4238', line: '#374033', grain: '#2f382c' },
  { label: '深緑', board: '#3f4f2c', dark: '#232c18', line: '#212b14', grain: '#1b230f' },
];

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

  /* 画面確認用の固定 (`KUROOBI_THEME=light`)。テーマの切り替えは
     設定 → 表示 にしかなく、撮るたびに人が押すしかなかった。
     **保存はしない**ので、次に普通に起動すれば好みどおりに戻る。 */
  const [forced, setForced] = useState<Theme | ''>('');
  useEffect(() => {
    void api.themeOverride()
      .then((t) => { if (t === 'light' || t === 'dark') setForced(t); })
      .catch(() => { /* Tauri 外や古いバイナリでは効かないだけ */ });
  }, []);

  // テーマは :root の属性で切り替える。`os` のときは属性を外して
  // prefers-color-scheme に任せる (tokens.css がその形で書いてある)
  useEffect(() => {
    const el = document.documentElement;
    const t = forced || prefs.theme;
    if (t === 'os') el.removeAttribute('data-theme');
    else el.setAttribute('data-theme', t);
  }, [prefs.theme, forced]);

  // 石返しの長さは CSS 変数。base.css の .k-flip がこれを見る
  useEffect(() => {
    document.documentElement.style.setProperty('--flip-dur', prefs.flipMs + 'ms');
  }, [prefs.flipMs]);

  /* 畳の色。トークンを上書きするので、テーマを切り替えても選んだ色が残る。
   * 標準 (0) のときは何も書かない — tokens.css の値をそのまま使わせる
   * (ライトはライトの緑を持っているので、上書きすると台無しになる)。 */
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

/** 盤を回すか。`auto` は自分の色が白のときだけ回す (自分を下に置く)。 */
export const flipped = (facing: Facing, myColor: 'black' | 'white' | ''): boolean =>
  facing === 'white' || (facing === 'auto' && myColor === 'white');

