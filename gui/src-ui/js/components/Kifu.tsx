// 棋譜。1 手 1 行の表で、評価値 (黒視点)・思考時間・出所 (定石/読切/探索) を
// 添える。指した側が大きく損した手には評価の下げ幅を赤で示す。
// 行をクリックするとその手を打った直後の局面へ移動する。先の手は薄く
// 残り、前へも戻れる。
//
// 評価値の出どころは 2 つ: エンジンが指したときの記録 (info) と、
// 評価値グラフの計算結果 (values)。記録がある手はそちらを優先する。
import { useEffect, useRef } from 'react';
import type { MoveInfo } from '../state';
import type { GraphPoint } from './Graph';

const sqName = (sq: number): string => 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);

export interface KifuProps {
  moves: (number | null)[];
  /** 今の局面が何手目の後か。 */
  cursor: number;
  /** 手数 → エンジンが指したときの記録 (評価値は黒視点)。 */
  info: Record<number, MoveInfo>;
  /** 評価値グラフの計算結果 (黒視点)。[n] = n 手目後の局面。 */
  values?: (GraphPoint | undefined)[] | null;
  onJump: (n: number) => void;
}

export function Kifu({ moves, cursor, info, values, onJump }: KifuProps) {
  // 現在手を掴んでおく。クラス名で探すと、装飾を変えたときに黙って壊れる。
  const current = useRef<HTMLTableRowElement>(null);

  // 現在手が隠れないように追う
  useEffect(() => {
    current.current?.scrollIntoView({ block: 'nearest' });
  }, [cursor, moves.length]);

  // 悪手マークは前の手との評価差から出すので、先に全手の表示値を並べる
  const shown = moves.map((_, i) =>
    info[i + 1] ? info[i + 1].value : values?.[i + 1]?.value);

  return (
    <div id="kifu">
      <table>
        <thead>
          <tr>
            <th className="n">#</th><th>手</th><th className="v">評価</th>
            <th className="t">時間</th><th>出所</th>
          </tr>
        </thead>
        <tbody>
          {moves.map((m, i) => {
            const n = i + 1;
            const rec = info[n];
            const gp = values?.[n];
            const value = shown[i];
            const exact = rec ? rec.exact : gp?.exact ?? false;
            const book = rec ? rec.source === 'book' : gp?.book ?? false;
            const src = value === undefined ? ''
              : book ? (rec?.learned ? '定石·学' : '定石')
              : exact ? '読切'
              : '探索';
            // 指した側から見て前の局面より 2 石以上損した手に印を付ける。
            // 値は黒視点なので、黒番 (奇数手) は下げ幅、白番は上げ幅が損
            const prev = i === 0 ? values?.[0]?.value ?? 0 : shown[i - 1];
            const loss = value !== undefined && prev !== undefined
              ? (i % 2 === 0 ? prev - value : value - prev)
              : 0;
            const cls = ['mv'];
            if (n === cursor) cls.push('current');
            if (n > cursor) cls.push('future');
            if (m == null) cls.push('pass');
            return (
              <tr
                key={n}
                ref={n === cursor ? current : undefined}
                className={cls.join(' ')}
                onClick={() => {
                  // 着地先の次の手がパスなら、打てない手番で止まらないよう進める
                  let to = n;
                  while (to < moves.length && moves[to] == null) to++;
                  onJump(to);
                }}
              >
                <td className="n">{n}</td>
                <td className="m">
                  {/* パス込みで 1 手ごとに手番が入れ替わるので、奇数手 = 黒 */}
                  <span className={'st ' + (i % 2 === 0 ? 'b' : 'w')} />
                  {m == null ? 'パス' : sqName(m)}
                </td>
                <td className="v">
                  {value !== undefined
                    ? (value > 0 ? '+' : '') + value.toFixed(exact ? 0 : 1)
                    : ''}
                  {loss >= 2 && <span className="dl">▼{loss.toFixed(1)}</span>}
                </td>
                <td className="t">{rec ? rec.secs.toFixed(1) : ''}</td>
                <td className={'s' + (book ? ' book' : exact ? ' exact' : '')}>{src}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
