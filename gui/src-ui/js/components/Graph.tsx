// 評価値グラフ (黒視点の折れ線)。検討モードで使う。
//
// 点にホバーすると値が出て、クリックでその局面へ飛ぶ。パスの手番は
// 測っても 0 が並ぶだけなので飛ばす。
import { useState } from 'react';

export interface GraphPoint { value: number; exact: boolean; book: boolean }

export interface GraphProps {
  /** [n] = n 手目まで進めた局面の評価。undefined は未計算。長さ moves.length+1 */
  values: (GraphPoint | undefined)[] | null;
  moves: (number | null)[];
  cursor: number;
  busy: boolean;
  onJump: (n: number) => void;
}

const W = 800, H = 190, L = 36, R = 16, T = 16, B = 24;

export function Graph({ values, moves, cursor, busy, onJump }: GraphProps) {
  const [tip, setTip] = useState<{ n: number; p: GraphPoint } | null>(null);
  const len = Math.max(1, moves.length);
  const vals = values ?? [];
  const defined = vals.filter((v): v is GraphPoint => !!v).map((v) => Math.abs(v.value));
  const ymax = Math.max(8, Math.min(64,
    Math.ceil((defined.length ? Math.max(...defined) : 0) / 8) * 8 || 8));
  const clamp = (v: number) => Math.max(-ymax, Math.min(ymax, v));
  const x = (n: number) => L + (W - L - R) * n / len;
  const y = (v: number) => T + (H - T - B) * (1 - (v + ymax) / (2 * ymax));

  const nearest = (ev: React.MouseEvent<SVGRectElement>): number | null => {
    const r = ev.currentTarget.ownerSVGElement!.getBoundingClientRect();
    const mx = (ev.clientX - r.left) * (W / r.width);
    let n = Math.round((mx - L) / ((W - L - R) / len));
    n = Math.max(0, Math.min(moves.length, n));
    return vals[n] ? n : null;
  };

  // 折れ線 (未計算の点は飛ばして繋ぐ)
  let d = '';
  let pen = false;
  vals.forEach((v, n) => {
    if (!v) return;
    d += (pen ? 'L' : 'M') + x(n).toFixed(1) + ' ' + y(clamp(v.value)).toFixed(1) + ' ';
    pen = true;
  });

  const grid = (v: number, label: string, strong: boolean) => (
    <g key={label}>
      <line x1={L} y1={y(v)} x2={W - R} y2={y(v)}
            stroke={strong ? '#3a4450' : '#272e37'} strokeWidth={1} />
      <text x={L - 4} y={y(v) + 3} textAnchor="end" fill="var(--sub)" fontSize={11}>{label}</text>
    </g>
  );

  return (
    <svg id="graph" viewBox={`0 0 ${W} ${H}`}>
      <rect x={0} y={0} width={W} height={H} rx={8} fill="#1a1f25" />
      {grid(ymax, '+' + ymax, false)}
      {grid(-ymax, '-' + ymax, false)}
      {grid(0, '0', true)}
      <text x={W - R} y={T + 7} textAnchor="end" fill="var(--sub)" fontSize={11}>黒有利</text>
      <text x={W - R} y={H - B - 2} textAnchor="end" fill="var(--sub)" fontSize={11}>白有利</text>
      {Array.from({ length: Math.floor(moves.length / 10) }, (_, k) => (k + 1) * 10).map((n) => (
        <text key={n} x={x(n)} y={H - 4} textAnchor="middle" fill="var(--sub)" fontSize={11}>{n}</text>
      ))}

      <line x1={x(cursor)} y1={T} x2={x(cursor)} y2={H - B}
            stroke="var(--accent-dim)" strokeWidth={1} strokeDasharray="3 3" />

      {defined.length === 0 ? (
        <text x={(L + W - R) / 2} y={H / 2 - 8} textAnchor="middle"
              fill="var(--sub)" fontSize={13}>
          {busy ? '計算中…' : '「更新」で全局面を採点します'}
        </text>
      ) : (
        <>
          <path d={d} fill="none" stroke="var(--accent)" strokeWidth={2} strokeLinejoin="round" />
          {vals.map((v, n) => v && (
            // 出所で塗り分け: 定石 = 金 / 読切 = 白 / 探索 = 青
            <circle key={n} cx={x(n)} cy={y(clamp(v.value))} r={v.exact || v.book ? 4 : 3}
                    fill={v.book ? 'var(--gold)' : v.exact ? '#e8ebee' : 'var(--accent)'}
                    stroke="#1a1f25" strokeWidth={1} />
          ))}
          {tip && (
            <>
              <circle cx={x(tip.n)} cy={y(clamp(tip.p.value))} r={6}
                      fill="none" stroke="#fff" strokeWidth={1.5} />
              <text fontSize={12} fill="var(--text)" textAnchor="middle"
                    x={Math.max(L + 20, Math.min(W - R - 20, x(tip.n)))}
                    y={y(clamp(tip.p.value)) < H / 2
                      ? y(clamp(tip.p.value)) + 14 : y(clamp(tip.p.value)) - 8}>
                {`${tip.n}手 ${tip.p.value > 0 ? '+' : ''}${
                  tip.p.exact ? tip.p.value.toFixed(0) : tip.p.value.toFixed(1)}${
                  tip.p.book ? ' (定石)' : tip.p.exact ? ' (読切)' : ''}`}
              </text>
            </>
          )}
        </>
      )}

      <rect x={L} y={0} width={W - L - R} height={H} fill="transparent" cursor="pointer"
            onMouseMove={(ev) => {
              const n = nearest(ev);
              const p = n === null ? undefined : vals[n];
              setTip(n !== null && p ? { n, p } : null);
            }}
            onMouseLeave={() => setTip(null)}
            onClick={(ev) => {
              const n0 = nearest(ev);
              if (n0 === null) return;
              let n = n0;
              while (n < moves.length && moves[n] == null) n++;
              onJump(n);
            }} />
    </svg>
  );
}
