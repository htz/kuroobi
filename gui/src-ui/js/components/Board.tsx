// 盤。石・合法手・最終手・評価値を描く。
//
// ローカル対局と GGS の対局で同じものを使う (今は 2 つのアプリで別々に
// 書いていて、片方だけ直す取りこぼしが起きている)。そのため盤は状態を
// 持たず、渡されたものを描くだけにしてある。
import type { Hints } from '../state';

const CELL = 100;
const PAD = 40;

export interface BoardProps {
  /** 64 マス: 0 空き / 1 黒 / 2 白 (file-major: sq = file*8 + rank)。 */
  cells: number[];
  /** 打てるマス。空なら合法手を出さない (観戦など)。 */
  legal?: number[];
  /** 直前の着手。丸で囲む。 */
  last?: number | null;
  /** 棋譜上でこの局面の次に実際に指された手。金の破線で示す。 */
  next?: number | null;
  /** 全合法手の評価値。あればマスに数字を出す。 */
  hints?: Hints | null;
  /** 押せない状態 (思考中・観戦中)。 */
  disabled?: boolean;
  onPlay?: (sq: number) => void;
}

const xy = (sq: number): [number, number] => [Math.floor(sq / 8), sq % 8];

export function Board(props: BoardProps) {
  const { cells, legal = [], last, next, hints, disabled, onPlay } = props;
  const legalSet = new Set(legal);
  const best = hints
    ? Math.max(...Object.values(hints).map((h) => h.value))
    : null;

  return (
    <svg className="board" viewBox="0 0 880 880">
      <rect x={0} y={0} width={880} height={880} rx={14} fill="#20252c" />
      <rect x={PAD - 6} y={PAD - 6} width={812} height={812} rx={6} fill="var(--board-dark)" />
      <rect x={PAD} y={PAD} width={800} height={800} fill="var(--board)" />

      {Array.from({ length: 9 }, (_, i) => (
        <g key={`grid${i}`}>
          <line x1={PAD + i * CELL} y1={PAD} x2={PAD + i * CELL} y2={PAD + 800}
                stroke="var(--line)" strokeWidth={2} />
          <line x1={PAD} y1={PAD + i * CELL} x2={PAD + 800} y2={PAD + i * CELL}
                stroke="var(--line)" strokeWidth={2} />
        </g>
      ))}

      {Array.from({ length: 8 }, (_, i) => (
        <g key={`lbl${i}`}>
          <text x={PAD + i * CELL + 50} y={27} textAnchor="middle"
                fill="var(--sub)" fontSize={19}>{'abcdefgh'[i]}</text>
          <text x={21} y={PAD + i * CELL + 57} textAnchor="middle"
                fill="var(--sub)" fontSize={19}>{i + 1}</text>
        </g>
      ))}

      {([[2, 2], [2, 6], [6, 2], [6, 6]] as [number, number][]).map(([fx, fy]) => (
        <circle key={`dot${fx}${fy}`} cx={PAD + fx * CELL} cy={PAD + fy * CELL}
                r={5} fill="var(--line)" />
      ))}

      {cells.map((v, sq) => {
        const [f, r] = xy(sq);
        const cx = PAD + f * CELL + 50;
        const cy = PAD + r * CELL + 50;

        if (v === 1 || v === 2) {
          return (
            <g key={sq}>
              {/* フラットな円盤。薄い落ち影と細いリムだけ */}
              <circle cx={cx} cy={cy + 2} r={40} fill="rgba(0,0,0,.25)" />
              <circle cx={cx} cy={cy} r={40}
                      fill={v === 1 ? '#111213' : '#f1f1f1'}
                      stroke={v === 1 ? '#2c2e30' : '#c9cdd1'} strokeWidth={2} />
              {last === sq && (
                <circle cx={cx} cy={cy} r={9} fill="none"
                        stroke="var(--accent)" strokeWidth={4} />
              )}
            </g>
          );
        }
        if (!legalSet.has(sq)) return null;

        const h = hints?.[sq];
        const isBest = h != null && best !== null && h.value >= best - 1e-6;
        return (
          <g key={sq} cursor={disabled ? 'default' : 'pointer'}
             onClick={disabled ? undefined : () => onPlay?.(sq)}>
            <circle cx={cx} cy={cy} r={46} fill="transparent" />
            {next === sq && (
              <circle cx={cx} cy={cy} r={38} fill="none" stroke="var(--gold)"
                      strokeWidth={2.5} strokeDasharray="6 5" />
            )}
            {h ? (
              <>
                <circle cx={cx} cy={cy} r={30}
                        fill={isBest ? 'rgba(255,212,121,.14)' : 'rgba(255,255,255,.06)'}
                        stroke={isBest ? 'var(--gold)' : 'rgba(255,255,255,.25)'}
                        strokeWidth={isBest ? 2 : 1} />
                <text x={cx} y={cy + (h.book ? 2 : 8)} textAnchor="middle" fontSize={24}
                      fill={isBest ? 'var(--gold)' : '#d9e6de'}
                      fontWeight={isBest ? 700 : 400}>
                  {(h.value > 0 ? '+' : '') + h.value.toFixed(h.exact ? 0 : 1)}
                </text>
                {/* 出所: 定石の値には印を付ける (探索値と混ざるため) */}
                {h.book && (
                  <text x={cx} y={cy + 22} textAnchor="middle" fontSize={13}
                        fill="var(--gold)">定石</text>
                )}
              </>
            ) : (
              <circle className="hint-dot" cx={cx} cy={cy} r={7}
                      fill="rgba(255,255,255,.30)" />
            )}
          </g>
        );
      })}
    </svg>
  );
}
