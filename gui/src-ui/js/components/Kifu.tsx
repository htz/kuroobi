// 棋譜。手をクリックするとその手を打った直後の局面へ移動する。
// 先の手は薄く残り、前へも戻れる。
import { useEffect, useRef } from 'react';
import type { MoveSource } from '../state';

const sqName = (sq: number): string => 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);

export interface KifuProps {
  moves: (number | null)[];
  /** 今の局面が何手目の後か。 */
  cursor: number;
  /** 手数 → その手の由来。エンジンが指した手だけ入る。 */
  source: Record<number, MoveSource>;
  onJump: (n: number) => void;
}

export function Kifu({ moves, cursor, source, onJump }: KifuProps) {
  // 現在手を掴んでおく。クラス名で探すと、装飾を変えたときに黙って壊れる。
  const current = useRef<HTMLSpanElement>(null);

  // 現在手が隠れないように追う
  useEffect(() => {
    current.current?.scrollIntoView({ block: 'nearest' });
  }, [cursor, moves.length]);

  return (
    <div id="kifu">
      {moves.map((m, i) => {
        const n = i + 1;
        const src = source[n];
        const cls = ['mv'];
        if (n === cursor) cls.push('current');
        if (n > cursor) cls.push('future');
        if (m == null) cls.push('pass');
        if (src) cls.push('src-' + src);
        return (
          <span
            key={n}
            ref={n === cursor ? current : undefined}
            className={cls.join(' ')}
            title={src ? (src === 'book' ? '定石どおりの手' : 'エンジンの探索による手') : undefined}
            onClick={() => {
              // 着地先の次の手がパスなら、打てない手番で止まらないよう進める
              let to = n;
              while (to < moves.length && moves[to] == null) to++;
              onJump(to);
            }}
          >
            <i>{n}</i>
            {/* パス込みで 1 手ごとに手番が入れ替わるので、奇数手 = 黒 */}
            <span className={'st ' + (i % 2 === 0 ? 'b' : 'w')} />
            {m == null ? 'ps' : sqName(m)}
          </span>
        );
      })}
    </div>
  );
}
