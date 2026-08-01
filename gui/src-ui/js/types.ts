// バックエンド (gui/src/main.rs) が返す型。Rust 側と 1 対 1 で対応させる。

export interface GameView {
  /** 64 マス: 0 空 / 1 黒 / 2 白 (A1=0 の file-major)。 */
  cells: number[];
  player: 'black' | 'white';
  legal: number[];
  black: number;
  white: number;
  over: boolean;
  last: number | null;
  kifu: string;
  move_count: number;
  /** 全手順 (undo で戻った先も含む)。null はパス。 */
  moves: (number | null)[];
  cursor: number;
}

export interface ThinkView {
  pos: number | null;
  value: number;
  exact: boolean;
  from_book: boolean;
  /** この手に使った時間 (秒)。 */
  secs: number;
}

export interface HintView {
  pos: number;
  value: number;
  exact: boolean;
}

export interface EvalPoint {
  n: number;
  /** 黒視点の石差。 */
  value: number;
  exact: boolean;
}

declare global {
  interface Window {
    __TAURI__?: {
      core: { invoke: <T = unknown>(cmd: string, args?: Record<string, unknown>) => Promise<T> };
    };
  }
}
