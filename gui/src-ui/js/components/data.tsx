import React from 'react';
import { Badge, Dot } from './primitives';

/* KUROOBI data — 棋譜表・評価値グラフ・対局者行・レート
 * 表は 1 行 --h-row。列幅は固定で、値は右揃え。
 * 損失（▼n）は評価値の専用列。出所列には入れない。
 */

/** 画面に描く石の色。エンジンの 1|2 は境目で変換して、画面まで持ち込まない
 *  （視点切り替えを入れるときに楽になる）。GGS へ送る色は ggs.ts の
 *  COLOR_CHOICES（* / O / ?）で、こちらとは別物。 */
export type StoneColor = 'b' | 'w';
export const toStoneColor = (n: 1 | 2): StoneColor => (n === 1 ? 'b' : 'w');

export type Move = {
  n: number; move: string; color: StoneColor;
  score?: number; loss?: number; secs?: number;
  /** 出所。「定石·学」は実戦から学習した枝 — 現行の棋譜表が区別しているので落とさない */
  src?: '定石' | '定石·学' | '探索' | '読切';
  /** 打てる手がなくて飛んだ手番。move は使わない */
  pass?: boolean;
};

export function KifuTable({ moves, current, onSelect }: { moves: Move[]; current?: number; onSelect?: (n: number) => void }) {
  // 現在手を追う。対局中は毎手増えるので、放っておくと下へ流れて見えなくなる。
  // scrollIntoView は祖先ごと動かしてしまう (パネル全体が流れて上の操作が
  // 画面外へ出る) ので、枠の中だけで計算する
  const box = React.useRef<HTMLDivElement>(null);
  const row = React.useRef<HTMLButtonElement>(null);
  React.useEffect(() => {
    const b = box.current, r = row.current;
    if (!b || !r) return;
    const top = r.offsetTop, bottom = top + r.offsetHeight;
    if (top < b.scrollTop) b.scrollTop = top;
    else if (bottom > b.scrollTop + b.clientHeight) b.scrollTop = bottom - b.clientHeight;
  }, [current, moves.length]);
  return (
    <div style={{ display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      <div style={{
        display: 'flex', gap: 'var(--sp-2)', height: 'var(--h-head)', flex: 'none', alignItems: 'center',
        padding: '0 var(--sp-3)', fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em',
        color: 'var(--sub)', borderBottom: '1px solid var(--border)',
      }}>
        <span style={{ width: 22, textAlign: 'right' }}>#</span>
        <span style={{ width: 58 }}>手</span>
        <span style={{ width: 56, textAlign: 'right' }}>評価</span>
        <span style={{ width: 34, textAlign: 'right' }}>時間</span>
        <span style={{ flex: 1, textAlign: 'right' }}>出所</span>
      </div>
      <div className="k-scroll" ref={box} style={{ flex: 1, minHeight: 0 }}>
        {moves.map(m => {
          const played = m.score !== undefined;
          const isCurrent = m.n === current;
          return (
            // 行は選択肢なので button。検討中はここを ↑↓ で送り続けるので、
            // 表に焦点が当たること自体が前提になる（div だと Tab で回ってこない）。
            <button key={m.n} type="button" onClick={() => onSelect?.(m.n)}
              ref={isCurrent ? row : undefined}
              aria-current={isCurrent || undefined}
              className={'k-row' + (isCurrent ? ' k-on' : '')} style={{
                width: '100%', border: 0, textAlign: 'left',
                display: 'flex', gap: 'var(--sp-2)', height: 'var(--h-row)', alignItems: 'center',
                padding: '0 var(--sp-3)', fontSize: 'var(--fs-5)',
                borderBottom: '1px solid var(--border-weak)',
                color: played ? 'var(--text)' : 'var(--sub)',
                background: isCurrent ? 'color-mix(in srgb, var(--accent) 14%, transparent)' : 'transparent',
                boxShadow: isCurrent ? 'inset 2px 0 0 var(--accent)' : 'none',
              }}>
              <span style={{ width: 22, textAlign: 'right', fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{m.n}</span>
              <span style={{ width: 58, display: 'flex', alignItems: 'center', gap: 6, fontFamily: 'var(--ff-mono)' }}>
                <StoneDot color={m.color} />
                {/* パスは座標ではないので等幅を外し、--sub で弱く出す
                    （着手の列に日本語が混ざるので、桁を揃えようとしない） */}
                {m.pass
                  ? <span style={{ fontFamily: 'inherit', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>パス</span>
                  : m.move}
              </span>
              <span style={{ width: 56, display: 'flex', justifyContent: 'flex-end', gap: 5 }}>
                {m.loss ? <span style={{ fontSize: 'var(--fs-7)', color: 'var(--bad)' }}>▼{m.loss}</span> : null}
                <span>{m.score === undefined ? '' : (m.score > 0 ? '+' : '') + m.score.toFixed(1)}</span>
              </span>
              <span style={{ width: 34, textAlign: 'right', color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>{m.secs?.toFixed(1) ?? ''}</span>
              <span style={{ flex: 1, textAlign: 'right', fontSize: 'var(--fs-6)', color: m.src?.startsWith('定石') ? 'var(--gold)' : m.src === '読切' ? 'var(--text)' : 'var(--sub)' }}>{m.src ?? ''}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

/* 石の点。表の行・対局者行・凡例で使う。盤の石は board.tsx の Stone */
export function StoneDot({ color, size = 9 }: { color: StoneColor; size?: number }) {
  const black = color === 'b';
  return <span style={{
    width: size, height: size, borderRadius: '50%', flex: 'none',
    background: black ? 'var(--stone-black)' : 'var(--stone-white)',
    boxShadow: 'inset 0 0 0 1px ' + (black ? 'var(--stone-black-edge)' : 'var(--stone-white-edge)'),
  }} />;
}

/* 箱に入れず、盤の下の帯として全幅に置く。
 * 縦軸は石差（0 が互角、上が黒有利、下が白有利）で 8 石ごとに破線、
 * 横軸は手数で 10 手ごとに罫線と目盛。どちらも省くと「何石差でどの手か」が読めない。
 * 点は出所で塗り分ける — 定石 --gold / 読切 --text / 探索 --accent。
 */
export type GraphPoint = { value: number; exact?: boolean; book?: boolean };

export function EvalGraph({ points, plies, cursor, blunder, busy, title = '評価値グラフ (黒視点)', extra, onJump }: {
  points: (GraphPoint | undefined)[];
  plies?: number;
  cursor?: number;
  blunder?: { at: number; loss: number };
  /** 分析中。未計算と区別するために中央の文言を変える */
  busy?: boolean;
  title?: string;
  /** 見出し行の右端（分析の進み具合・停止ボタンなど） */
  extra?: React.ReactNode;
  /** 押した手数へ飛ぶ。現行にある操作なので落とさない */
  onJump?: (n: number) => void;
}) {
  const W = 800, H = 210, L = 44, R = 54, T = 18, B = 26, STEP = 8;
  const len = Math.max(1, plies ?? points.length - 1);
  const defined = points.filter((p): p is GraphPoint => !!p).map(p => Math.abs(p.value));
  const ymax = Math.max(STEP, Math.min(64, Math.ceil((defined.length ? Math.max(...defined) : 0) / STEP) * STEP));
  const clamp = (v: number) => Math.max(-ymax, Math.min(ymax, v));
  const x = (n: number) => L + (W - L - R) * n / len;
  const y = (v: number) => T + (H - T - B) * (1 - (v + ymax) / (2 * ymax));

  const rows: number[] = [];
  for (let v = -ymax; v <= ymax; v += STEP) rows.push(v);
  const cols: number[] = [];
  for (let n = 0; n <= len; n += 10) cols.push(n);

  let d = '', pen = false;
  points.forEach((p, n) => {
    if (!p) return;
    d += (pen ? 'L' : 'M') + x(n).toFixed(1) + ' ' + y(clamp(p.value)).toFixed(1) + ' ';
    pen = true;
  });

  // 敗着は終盤に出るほうが多い。ラベルは 90px 前後あるので、
  // 右に寄ったら線の左側へ回す（そのままだと viewBox から切れる）。
  const bx = blunder ? x(blunder.at) : 0;
  const bRight = blunder ? bx > W - R - 100 : false;

  return (
    <div style={{
      flex: 'none', borderTop: '1px solid var(--border)', background: 'var(--panel)',
      padding: 'var(--sp-3) var(--sp-4) var(--sp-4)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
    }}>
      {/* 凡例は必ずグラフの外（見出し行）に置く — 描画領域に重ねると
          データと衝突するし、縮小されて読めなくなる */}
      <div style={{
        height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        fontSize: 'var(--fs-6)', color: 'var(--sub)',
      }}>
        <span>{title}</span>
        <Legend tone="gold">定石</Legend>
        <Legend tone="text">読切</Legend>
        <Legend tone="accent">探索</Legend>
        {extra && <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>{extra}</span>}
      </div>
      <div style={{ position: 'relative' }}>
        {/* 押した位置の手数へ飛ぶ。viewBox は器より広いので、実寸の比で数え直す */}
        <svg viewBox={`0 0 ${W} ${H}`} style={{ width: '100%', height: 'auto', display: 'block' }}
             role="img" aria-label="評価値グラフ（縦軸 石差、横軸 手数）"
             onClick={onJump && (e => {
               const r = e.currentTarget.getBoundingClientRect();
               const px = ((e.clientX - r.left) / r.width) * W;
               const n = Math.round(((px - L) / (W - L - R)) * len);
               if (n >= 0 && n <= len) onJump(n);
             })}>
          <rect x={0} y={0} width={W} height={H} rx={8} fill="var(--bg)" />
          {/* 単位は目盛の列に入れない（+16 と重なる）。左上に逃がす */}
          <text x={10} y={12} fill="var(--sub)" fontSize={10}>石差</text>

          {rows.map(v => (
            <g key={'r' + v}>
              <line x1={L} y1={y(v)} x2={W - R} y2={y(v)}
                    stroke={v === 0 ? 'var(--border)' : 'var(--border-weak)'} strokeWidth={1}
                    strokeDasharray={v === 0 ? undefined : '2 3'} />
              <text x={L - 6} y={y(v) + 4} textAnchor="end" fill="var(--sub)" fontSize={11}>
                {v > 0 ? '+' + v : v === 0 ? '0' : '−' + -v}
              </text>
            </g>
          ))}
          <text x={W - R + 8} y={y(ymax) + 12} fill="var(--sub)" fontSize={11}>黒有利</text>
          <text x={W - R + 8} y={y(0) + 4} fill="var(--sub)" fontSize={11}>互角</text>
          <text x={W - R + 8} y={y(-ymax) - 6} fill="var(--sub)" fontSize={11}>白有利</text>

          {cols.map(n => (
            <g key={'c' + n}>
              {n > 0 && <line x1={x(n)} y1={T} x2={x(n)} y2={H - B} stroke="var(--border-weak)" strokeWidth={1} />}
              <text x={x(n)} y={H - 8} textAnchor="middle" fill="var(--sub)" fontSize={11}>{n}</text>
            </g>
          ))}
          <text x={W - R + 8} y={H - 8} fill="var(--sub)" fontSize={10}>手</text>

          {d && <path d={d.trim()} fill="none" stroke="var(--accent)" strokeWidth={2} strokeLinejoin="round" />}

          {blunder && <>
            <line x1={bx} y1={T} x2={bx} y2={H - B} stroke="var(--bad)" strokeWidth={1.5} />
            <text x={bRight ? bx - 7 : bx + 7} y={T + 11} textAnchor={bRight ? 'end' : 'start'}
                  fill="var(--bad)" fontSize={11}>
              {blunder.at} 手 敗着 ▼{blunder.loss}
            </text>
          </>}
          {cursor !== undefined && (
            <line x1={x(cursor)} y1={T} x2={x(cursor)} y2={H - B}
                  stroke="var(--accent-dim)" strokeWidth={1} strokeDasharray="3 3" />
          )}

          {points.map((p, n) => p && (
            <circle key={n} cx={x(n)} cy={y(clamp(p.value))} r={p.exact || p.book ? 4 : 3}
                    fill={p.book ? 'var(--gold)' : p.exact ? 'var(--text)' : 'var(--accent)'}
                    stroke="var(--bg)" strokeWidth={1} />
          ))}
        </svg>
        {/* 線が無いグラフをそのまま出すと、壊れているのか未計算なのか分からない */}
        {!d && (
          <div style={{
            position: 'absolute', inset: 0, display: 'grid', placeItems: 'center',
            fontSize: 'var(--fs-5)', color: 'var(--sub)',
          }}>{busy ? '分析中…' : '「分析」で評価値を出します'}</div>
        )}
      </div>
    </div>
  );
}

function Legend({ tone, children }: { tone: 'gold' | 'text' | 'accent'; children: React.ReactNode }) {
  return (
    <span style={{ display: 'flex', alignItems: 'center', gap: 5 }}>
      <span style={{ width: 7, height: 7, borderRadius: '50%', background: `var(--${tone})` }} />
      {children}
    </span>
  );
}

/* 対局者行。盤の上下に密着させる。人間・エンジン・GGS で同じ部品を使う。
 * 高さは --h-field（32px）で統一 — 対局画面と GGS で行の高さが変わらないように。
 *
 * rate に添える dev（±偏差）は GGS のときだけ渡す。偏差が大きいと数字が
 * 意味を持たないので、GGS では省かないこと。
 * name を押せるようにするのは GGS だけ（プロフィールへ飛ぶ）。
 */
export function PlayerRow({ color, name, rate, dev, meta, clock, active, discs, onName }: {
  color: StoneColor; name: string;
  rate?: number; dev?: number; meta?: React.ReactNode;
  clock?: string; active?: boolean; discs?: number;
  onName?: () => void;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-field)', fontSize: 'var(--fs-5)' }}>
      <StoneDot color={color} size={14} />
      {/* 押せないときは span。button のままだと Tab で回ってくるのに何も起きない */}
      {onName
        ? <button type="button" onClick={onName} className="k-link" style={{
            border: 0, background: 'transparent', padding: 0, fontSize: 'var(--fs-4)', fontWeight: 600, color: 'var(--text)',
            borderBottom: '1px solid color-mix(in srgb, var(--accent) 40%, transparent)',
          }}>{name}</button>
        : <b style={{ fontSize: 'var(--fs-4)', fontWeight: 600 }}>{name}</b>}
      {rate !== undefined && (
        <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>
          {rate.toFixed(1)}{dev !== undefined && <span style={{ opacity: .7, marginLeft: 4 }}>±{dev}</span>}
        </span>
      )}
      {meta && <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>{meta}</span>}
      {discs !== undefined && <span style={{ fontSize: 'var(--fs-1)', fontWeight: 700 }}>{discs}</span>}
      {clock && <span style={{
        marginLeft: 'auto', fontSize: 'var(--fs-4)', fontWeight: active ? 700 : 400,
        padding: '3px 12px', borderRadius: 'var(--r-pill)',
        background: active ? 'var(--accent)' : 'var(--bg)',
        color: active ? 'var(--on-accent)' : 'var(--sub)',
      }}>{clock}</span>}
    </div>
  );
}

/* レート推移。ポップオーバーの中だけに置き、常設しない。
 *
 * ここは「上がっているか下がっているか」が分かれば足る場所なので、軸は省く。
 * 目盛を出すなら EvalGraph と同じ作りにすること（罫線 3 本で軸のつもりをするのは
 * 一番悪い）。現在値は隣の RateRow が出すものとしてここには置かない。
 *
 * ⚠ viewBox の幅は、入れる器の実寸幅と同じかそれより狭くすること。
 * 広すぎると svg 全体が縮小され、font-size 10 が 6px になって読めなくなる
 * （トークンの下限は --fs-7 = 10px）。このグラフはドック幅 268 を前提に 300。
 */
export function RateChart({ points, height = 74, width = 300 }: {
  points: number[];
  height?: number;
  /** viewBox の幅。**器の実寸に合わせる** — 300 のまま広い器へ置くと、
   *  preserveAspectRatio="none" で横に引き伸ばされて線の太さが崩れる。 */
  width?: number;
}) {
  // 新規プレイヤーや初対局前は必ず空。点が 1 個だと線が引けない
  if (points.length < 2) {
    return (
      <div style={{ height, display: 'grid', placeItems: 'center', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        まだ記録がありません
      </div>
    );
  }
  const w = width, pad = 8;
  const min = Math.min(...points), max = Math.max(...points), span = Math.max(1, max - min);
  const y = (p: number) => height - pad - ((p - min) / span) * (height - pad * 2 - 4);
  const xy = points.map((p, i) => [(i / (points.length - 1)) * w, y(p)] as const);
  const last = xy[xy.length - 1];
  return (
    <svg viewBox={'0 0 ' + w + ' ' + height} preserveAspectRatio="none" style={{ width: '100%', height, display: 'block' }}>
      {/* 罫線は height から出す — 固定値にすると height を変えた瞬間に枠からずれる */}
      {[0.25, 0.5, 0.75].map(t => {
        const gy = pad + t * (height - pad * 2);
        return <line key={t} x1={0} y1={gy} x2={w} y2={gy} stroke="var(--border-weak)" strokeWidth={1} />;
      })}
      <polyline points={xy.map(([px, py]) => px.toFixed(1) + ',' + py.toFixed(1)).join(' ')} fill="none" stroke="var(--accent)" strokeWidth={1.6} />
      <circle cx={last[0]} cy={last[1]} r={3} fill="var(--accent)" />
    </svg>
  );
}

export function ResultRow({ win, draw, opponent, discs, when, note, rating, dim, onClick }: {
  win: boolean;
  /** 引き分け。勝/負の 2 値に丸めると石差 0 の対局が「負」になる */
  draw?: boolean;
  opponent: string;
  discs: number;
  /** 終わった時刻 (表示用の文字列)。 */
  when?: string;
  /** 対局の形式など、名前の次に置く補足。 */
  note?: string;
  /** 対局後の自分のレート。 */
  rating?: number | null;
  /** 棋譜がどこにも無くて開けない対局。押せないことを見た目でも言う。 */
  dim?: boolean;
  onClick?: () => void;
}) {
  const body = <>
    <span style={{ width: 24, flex: 'none', color: draw ? 'var(--sub)' : win ? 'var(--ok)' : 'var(--bad)' }}>
      {draw ? '分' : win ? '勝' : '負'}
    </span>
    <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
      {opponent}
    </span>
    {note && (
      <span style={{ width: 'var(--w-gtype)', flex: 'none', color: 'var(--sub)',
                     fontSize: 'var(--fs-6)', overflow: 'hidden', textOverflow: 'ellipsis',
                     whiteSpace: 'nowrap' }}>{note}</span>
    )}
    <span style={{ width: 44, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                   fontVariantNumeric: 'tabular-nums' }}>
      {discs > 0 ? '+' + discs : discs}
    </span>
    {rating != null && (
      <span style={{ width: 60, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                     fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>
        {rating.toFixed(1)}
      </span>
    )}
    {when && (
      <span style={{ width: 64, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                     fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>{when}</span>
    )}
  </>;
  const style: React.CSSProperties = {
    display: 'flex', gap: 'var(--sp-2)', height: 'var(--h-field)', alignItems: 'center',
    width: '100%', fontSize: 'var(--fs-5)', borderBottom: '1px solid var(--border-weak)',
    padding: '0 var(--sp-2)', textAlign: 'left', color: 'var(--text)',
  };
  // 押せないときは span。button のままだと Tab で回ってくるのに何も起きない
  return onClick && !dim
    ? <button type="button" className="k-row" onClick={onClick} title="検討で開く"
              style={{ ...style, border: 0, background: 'transparent', cursor: 'pointer' }}>{body}</button>
    : <div style={{ ...style, opacity: dim ? 0.5 : 1 }}>{body}</div>;
}

export { Badge, Dot };

/* 手数を辿る帯。盤の下に全幅で置く。
 *
 * 評価値グラフでも飛べるが、あれは**分析したあと**にしか点が無い。
 * 読み込んだだけの棋譜を辿る道が「戻る／進む」の連打しか無かった。
 *
 * 掴めるように見えるものは本当に掴めること (掴めるのに動かないのは
 * 無いより悪い) — 押しても掴んで滑らせても同じところへ行く。
 */
export function MoveScrub({ plies, cursor, blunder, onSeek }: {
  /** 全手数。0 なら帯は出さない */
  plies: number;
  cursor: number;
  /** 敗着。盤に触らずに場所が分かるように赤い印を打つ */
  blunder?: { at: number; loss: number };
  onSeek: (n: number) => void;
}) {
  const box = React.useRef<HTMLDivElement>(null);
  if (plies <= 0) return null;

  const at = (n: number) => (n / plies) * 100;
  // 押した場所・滑らせた場所からいちばん近い手数へ。端は丸めずに 0 と plies
  const seekAt = (clientX: number) => {
    const r = box.current?.getBoundingClientRect();
    if (!r || r.width <= 0) return;
    const t = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    onSeek(Math.round(t * plies));
  };

  // 刻みは 1 手ごと。10 手ごとだけ長くして数字を添える
  const ticks = Array.from({ length: plies + 1 }, (_, i) => i);

  return (
    <div style={{ padding: '0 var(--sp-4)', flex: 'none' }}>
      <div ref={box} role="slider" aria-label="手数" aria-valuemin={0}
           aria-valuemax={plies} aria-valuenow={cursor} tabIndex={0}
           onPointerDown={(e) => {
             (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
             seekAt(e.clientX);
           }}
           onPointerMove={(e) => { if (e.buttons & 1) seekAt(e.clientX); }}
           style={{
             position: 'relative', height: 'var(--h-scrub)', width: '100%',
             cursor: 'pointer', touchAction: 'none', userSelect: 'none',
           }}>
        {/* 溝と、いまいるところまでの塗り */}
        <div style={{
          position: 'absolute', left: 0, right: 0, top: 11, height: 2,
          background: 'var(--track)', borderRadius: 1,
        }} />
        <div style={{
          position: 'absolute', left: 0, width: at(cursor) + '%', top: 11, height: 2,
          background: 'var(--accent)', borderRadius: 1,
        }} />

        {ticks.map((i) => {
          const ten = i % 10 === 0;
          return (
            <div key={i} style={{
              position: 'absolute', left: at(i) + '%', top: ten ? 6 : 8,
              width: 1, height: ten ? 12 : 8, marginLeft: -0.5,
              background: ten ? 'var(--border)' : 'var(--border-weak)',
            }} />
          );
        })}

        {/* 敗着。刻みより太く・赤く、上まで伸ばす */}
        {blunder && blunder.at <= plies && (
          <div title={`${blunder.at} 手 敗着 ▼${blunder.loss}`} style={{
            position: 'absolute', left: at(blunder.at) + '%', top: 3,
            width: 2, height: 18, marginLeft: -1, background: 'var(--bad)',
          }} />
        )}

        {ticks.filter((i) => i % 10 === 0).map((i) => (
          <span key={'n' + i} style={{
            position: 'absolute', left: at(i) + '%', top: 20,
            transform: 'translateX(-50%)',
            fontSize: 'var(--fs-7)', color: 'var(--sub)', lineHeight: 1,
          }}>{i}</span>
        ))}

        {/* 掴む丸。盤の石と同じ色づかいにすると盤面と読み違えるので、
            面は --card・縁は --accent にして「操作するもの」に見せる */}
        <div style={{
          position: 'absolute', left: at(cursor) + '%', top: 4,
          width: 16, height: 16, marginLeft: -8, borderRadius: '50%',
          background: 'var(--card)', border: '2px solid var(--accent)',
          boxShadow: 'var(--sh-1)', pointerEvents: 'none',
        }} />
      </div>
    </div>
  );
}
