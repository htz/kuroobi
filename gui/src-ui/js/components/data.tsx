import React from 'react';
import { Badge, Button, Dot } from './primitives';
import { Divider, Empty, TableHead, TableRow } from './layout';

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

export function KifuTable({ moves, current, onSelect, decimals = 1 }: {
  moves: Move[]; current?: number; onSelect?: (n: number) => void;
  /** 評価値の小数桁 (設定の 表示 タブ)。 */
  decimals?: number;
}) {
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
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      <TableHead>
        <span style={{ width: 22, textAlign: 'right' }}>#</span>
        <span style={{ width: 58 }}>手</span>
        <span style={{ width: 56, textAlign: 'right' }}>評価</span>
        <span style={{ width: 34, textAlign: 'right' }}>時間</span>
        <span style={{ flex: 1, textAlign: 'right' }}>出所</span>
      </TableHead>
      <div className="k-scroll" ref={box} style={{ flex: 1, minHeight: 0 }}>
        {/* **1 手も無いときに列見出しだけを残さない。**対局を始める前の
            画面はここが必ず空で、いちばん最初に目に入る枠が「壊れている
            のか、まだ何も無いのか」判らなかった */}
        {!moves.length && (
          <span style={{ display: 'block', padding: '0 var(--sp-3)' }}>
            <Empty>まだ手がありません。</Empty>
          </span>
        )}
        {moves.map(m => {
          const played = m.score !== undefined;
          const isCurrent = m.n === current;
          return (
            // 行は選択肢なので button。検討中はここを ↑↓ で送り続けるので、
            // 表に焦点が当たること自体が前提になる（div だと Tab で回ってこない）。
            // 評価と時間は縦に並ぶ数字の列。桁が揃わないと読み比べられない
            <TableRow key={m.n} on={isCurrent} muted={!played}
                      innerRef={isCurrent ? row : undefined}
                      onClick={() => onSelect?.(m.n)}>
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
                <span>{m.score === undefined ? '' : (m.score > 0 ? '+' : '') + m.score.toFixed(decimals)}</span>
              </span>
              <span style={{ width: 34, textAlign: 'right', color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>{m.secs?.toFixed(1) ?? ''}</span>
              <span style={{ flex: 1, textAlign: 'right', fontSize: 'var(--fs-6)', color: m.src?.startsWith('定石') ? 'var(--gold)' : m.src === '読切' ? 'var(--text)' : 'var(--sub)' }}>{m.src ?? ''}</span>
            </TableRow>
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

export function EvalGraph({ points, plies, cursor, blunder, busy, pov = 'b', extra, onJump, moveName, open }: {
  points: (GraphPoint | undefined)[];
  plies?: number;
  cursor?: number;
  blunder?: { at: number; loss: number };
  /** 分析中。未計算と区別するために中央の文言を変える */
  busy?: boolean;
  /** 見る側。見出しと上下の名札が入れ替わる (値は呼ぶ側が反して渡す) */
  pov?: 'b' | 'w';
  /** 見出し行の右端（分析の進み具合・停止ボタンなど） */
  extra?: React.ReactNode;
  /** 押した手数へ飛ぶ。現行にある操作なので落とさない */
  onJump?: (n: number) => void;
  /** その手数に指された手の名前 ("f5")。見出し行の読み取りに添える。 */
  moveName?: (n: number) => string | undefined;
  /** いちばん低い窓 (620px 以下) で開いているか。畳む段は base.css が持つ。
   *  開閉の釦は Toolbar 側 — 畳むと見出し行ごと消えるので中には置けない。 */
  open?: boolean;
}) {
  /* 触れているところの手数。**押さなくても読める**ようにする —
     点の位置だけでは何手目の何石差か分からず、いちいち押して盤を動かす
     しかなかった。出す場所は見出し行 (規則 19 — 描画領域に重ねない)。 */
  const [hover, setHover] = React.useState<number | null>(null);
  /* 器の実寸。**viewBox の高さを器から決める** — 幅 100% / 高さ auto だと
     縦横比が固定され、窓を低くしても帯が 266px のまま縮まず、盤が
     82px まで潰れていた (規則 7 は盤を最後まで残すと決めている)。
     幅の倍率 (実寸 / W) はそのままなので、**字も余白も大きさが変わらず**
     描画に使う縦だけが減る。ResizeObserver は ref のコールバックで張る
     (effect の中で setState すると React Compiler の lint が落ちる)。 */
  const [box, setBox] = React.useState({ w: 0, h: 0 });
  const obs = React.useRef<ResizeObserver | null>(null);
  const attach = React.useCallback((el: HTMLDivElement | null) => {
    obs.current?.disconnect();
    obs.current = null;
    if (!el) return;
    const o = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      setBox((b) => (Math.abs(b.w - r.width) < 0.5 && Math.abs(b.h - r.height) < 0.5
        ? b : { w: r.width, h: r.height }));
    });
    o.observe(el);
    obs.current = o;
  }, []);

  /* 見る側。**値は呼ぶ側が反して渡す** — 反す場所を 1 か所 (adapt) に
     まとめないと、棋譜表とグラフで符号がずれる */
  const up = pov === 'b' ? '黒' : '白';
  const down = pov === 'b' ? '白' : '黒';
  const title = `評価値グラフ (${up}視点)`;
  const W = 800, L = 44, R = 54, T = 18, B = 26, STEP = 8;
  /** 自然な高さと、これ以上は潰さない下限 */
  const NAT = 210, MIN = 120;
  const scale = box.w > 0 ? box.w / W : 0;
  /* 上限・下限は**器の側**に px で置く (倍率は幅だけで決まるので循環しない)。
     H を後から丸めると縦横で倍率がずれて字が歪む */
  const H = scale > 0 ? box.h / scale : NAT;
  const len = Math.max(1, plies ?? points.length - 1);
  const defined = points.filter((p): p is GraphPoint => !!p).map(p => Math.abs(p.value));
  const ymax = Math.max(STEP, Math.min(64, Math.ceil((defined.length ? Math.max(...defined) : 0) / STEP) * STEP));
  const clamp = (v: number) => Math.max(-ymax, Math.min(ymax, v));
  const x = (n: number) => L + (W - L - R) * n / len;
  const y = (v: number) => T + (H - T - B) * (1 - (v + ymax) / (2 * ymax));

  /* 罫線の間隔。**帯が低いと 8 石ごとでは目盛が重なって読めない** —
     字は倍率が同じで縮まないので、間隔のほうを間引く (16 → 32 → 64 石) */
  const rowStep = [1, 2, 4, 8].map(k => STEP * k)
    .find(s => (H - T - B) * s / (2 * ymax) >= 14) ?? STEP * 8;
  const rows: number[] = [];
  for (let v = -ymax; v <= ymax; v += rowStep) rows.push(v);
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
  // 敗着の手の値が上半分にあるなら、ラベルは下端へ回す (逆も同じ)。
  // 敗着はたいてい形勢が大きく動いた所なので、線は端に寄っていることが多い
  const bVal = blunder ? points[blunder.at]?.value : undefined;
  const bHigh = bVal !== undefined && bVal > 0;

  /** 器の中の x から手数を出す。viewBox は器より広いので実寸の比で数え直す */
  const plyAt = (e: React.MouseEvent<SVGSVGElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const px = ((e.clientX - r.left) / r.width) * W;
    const n = Math.round(((px - L) / (W - L - R)) * len);
    return n >= 0 && n <= len ? n : null;
  };
  const shown = hover !== null ? points[hover] : undefined;

  return (
    /* display はここに書かない。畳む段 (max-height 620) が動かすので、
       インラインに書くと media query が届かない — base.css の警告どおり、
       実際に「入口は出るのにグラフが消えない」状態になった */
    <div className={'k-graph' + (open ? ' k-open' : '')} style={{
      /* 縮む。盤より先に畳む側なので `none` にしない (規則 7) */
      flex: '0 1 auto', minHeight: 0,
      borderTop: '1px solid var(--border)', background: 'var(--panel)',
      padding: 'var(--sp-3) var(--sp-4) var(--sp-4)', flexDirection: 'column', gap: 'var(--sp-2)',
    }}>
      {/* 凡例は必ずグラフの外（見出し行）に置く — 描画領域に重ねると
          データと衝突するし、縮小されて読めなくなる */}
      {/* 操作が乗るときは帯を高くする。--h-head (20px) のままだと押せるものが
          罫にめり込み、当たりも文字ぶんしか無くなる (Section の aside と同じ話) */}
      <div style={{
        minHeight: extra ? 'var(--h-field)' : 'var(--h-head)',
        display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        fontSize: 'var(--fs-6)', color: 'var(--sub)',
      }}>
        <span>{title}</span>
        <Legend tone="gold">定石</Legend>
        <Legend tone="text">読切</Legend>
        <Legend tone="accent">探索</Legend>
        {/* 触れているところの読み取り。**幅の決まった枠に入れる** —
            文字数で凡例が左右に動くと、目で追う先が毎回変わる */}
        <span style={{ minWidth: 132, color: 'var(--text)', whiteSpace: 'nowrap' }}>
          {hover !== null && shown && <>
            {hover} 手{moveName?.(hover) ? ' ' + moveName(hover) : ''}
            {' '}<b style={{ fontWeight: 600 }}>
              {shown.value > 0 ? '+' : ''}{shown.value.toFixed(1)}
            </b>
            <span style={{ color: shown.book ? 'var(--gold)' : 'var(--sub)' }}>
              {' '}{shown.book ? '定石' : shown.exact ? '読切' : '探索'}
            </span>
          </>}
        </span>
        {extra && <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>{extra}</span>}
      </div>
      {/* 基準の高さを**明示する**。svg を height:100% にした時点で中身の高さが
          0 になり、auto のままだと帯が最小のまま伸びなくなる (窓が広くても
          グラフが潰れる)。基準 = 自然な高さ、下限まで縮む */}
      <div ref={attach} style={{
        position: 'relative', flexGrow: 0, flexShrink: 1,
        flexBasis: (scale ? NAT * scale : NAT) + 'px',
        minHeight: scale ? MIN * scale : MIN,
      }}>
        {/* 押した位置の手数へ飛ぶ。viewBox は器より広いので、実寸の比で数え直す */}
        <svg viewBox={`0 0 ${W} ${H.toFixed(2)}`} preserveAspectRatio="none"
             style={{ width: '100%', height: '100%', display: 'block' }}
             role="img" aria-label="評価値グラフ（縦軸 石差、横軸 手数）"
             onMouseMove={(e) => setHover(plyAt(e))}
             onMouseLeave={() => setHover(null)}
             onClick={onJump && ((e) => { const n = plyAt(e); if (n !== null) onJump(n); })}>
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
          {/* 上下の名札も見る側で入れ替える。値だけ反して名札を残すと、
              白視点で「上が黒有利なのに白が良いほど上へ伸びる」になる */}
          <text x={W - R + 8} y={y(ymax) + 12} fill="var(--sub)" fontSize={11}>{up}有利</text>
          <text x={W - R + 8} y={y(0) + 4} fill="var(--sub)" fontSize={11}>互角</text>
          <text x={W - R + 8} y={y(-ymax) - 6} fill="var(--sub)" fontSize={11}>{down}有利</text>

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
            {/* 上端に固定すると、線が上に張り付いている局面 (大差) でデータと
                重なる。**線の反対側へ回す** — 規則 52 を縦にも当てはめる */}
            <text x={bRight ? bx - 7 : bx + 7} y={bHigh ? H - B - 6 : T + 11}
                  textAnchor={bRight ? 'end' : 'start'} fill="var(--bad)" fontSize={11}>
              {blunder.at} 手 敗着 ▼{blunder.loss}
            </text>
          </>}
          {cursor !== undefined && (
            <line x1={x(cursor)} y1={T} x2={x(cursor)} y2={H - B}
                  stroke="var(--accent-dim)" strokeWidth={1} strokeDasharray="3 3" />
          )}

          {points.map((p, n) => p && (
            <circle key={n} cx={x(n)} cy={y(clamp(p.value))}
                    r={n === hover ? 5 : p.exact || p.book ? 4 : 3}
                    fill={p.book ? 'var(--gold)' : p.exact ? 'var(--text)' : 'var(--accent)'}
                    stroke="var(--bg)" strokeWidth={1} />
          ))}
          {/* 触れているところの縦線。いまいる手数の線 (--accent-dim の破線) と
              見分けが付くよう、細い実線にする */}
          {hover !== null && shown && (
            <line x1={x(hover)} y1={T} x2={x(hover)} y2={H - B}
                  stroke="var(--sub)" strokeWidth={1} />
          )}
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
/* 盤の下の 1 行。石数と持ち時間を左右に置く。
 *
 * **上下に 1 人ずつ置かない。** 盤を上下から挟むと、窓の高さから盤の大きさを
 * 決める (規則 77) ときに 2 行ぶん取られるうえ、視線が盤をまたいで往復する。
 * 数は並べたほうが差が読める。手番は石を明るくして示す。
 *
 * 寸法は設計の実測 — 帯は --h-bar (44px)、上に --border-weak の罫、
 * 左右 --sp-4、要素の間 20px、石は 14px、数は --fs-1 の 700。 */
export function ScoreRow({ black, white, turn, meta, blackClock, whiteClock }: {
  black: number; white: number;
  /** 手番。終局後は undefined。 */
  turn?: 'b' | 'w';
  meta?: React.ReactNode;
  blackClock?: string; whiteClock?: string;
}) {
  const side = (c: 'b' | 'w', n: number) => (
    <span style={{
      display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
      opacity: turn === undefined || turn === c ? 1 : 0.55,
    }}>
      <StoneDot color={c} size={14} />
      <b style={{ fontSize: 'var(--fs-1)', fontWeight: 700, color: 'var(--text)' }}>{n}</b>
    </span>
  );
  return (
    <div style={{
      flex: 'none', height: 'var(--h-bar)', display: 'flex', alignItems: 'center',
      padding: '0 var(--sp-4)', gap: 20, borderTop: '1px solid var(--border-weak)',
      fontSize: 'var(--fs-5)', color: 'var(--sub)',
    }}>
      {side('b', black)}
      {side('w', white)}
      {meta && <>
        <Divider />
        <span>{meta}</span>
      </>}
      {/* 時計は 1 秒ごとに書き換わる。桁が動くと石数まで揺れる */}
      {(blackClock || whiteClock) && (
        <span style={{ marginLeft: 'auto', display: 'flex', gap: 'var(--sp-4)', fontVariantNumeric: 'tabular-nums' }}>
          {blackClock && <span>黒 <b style={{ color: 'var(--text)', fontWeight: 600 }}>{blackClock}</b></span>}
          {whiteClock && <span>白 <b style={{ color: 'var(--text)', fontWeight: 600 }}>{whiteClock}</b></span>}
        </span>
      )}
    </div>
  );
}

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
        <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>
          {rate.toFixed(1)}{dev !== undefined && <span style={{ opacity: .7, marginLeft: 4 }}>±{dev}</span>}
        </span>
      )}
      {/* 石数は名前の隣に固定する。**長さの変わる meta を左に置くと、常時
          出ている石数が左右に動いて目障りになる。** meta は右へ回す */}
      {discs !== undefined && <span style={{ fontSize: 'var(--fs-1)', fontWeight: 700 }}>{discs}</span>}
      <span style={{ flex: 1 }} />
      {meta && <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>{meta}</span>}
      {clock && <span style={{
        fontSize: 'var(--fs-4)', fontWeight: active ? 700 : 400,
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
    /* **罫線は引かない (規則 56)。** 「上がっているか下がっているか」が
       分かれば足る場所なので軸は省くと決めてあり、規則は
       「罫線 3 本で軸のつもりをするのが一番悪い」と名指ししている。
       目盛の無い線は「読める気がするのに読めない」だけ。現在値は隣の
       `RateRow` が出す。目盛を出すなら `EvalGraph` と同じ作りにすること。 */
    <svg viewBox={'0 0 ' + w + ' ' + height} preserveAspectRatio="none" style={{ width: '100%', height, display: 'block' }}>
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
export function MoveScrub({ plies, cursor, blunder, onSeek, nav = true }: {
  /** 全手数。0 なら帯は出さない */
  plies: number;
  cursor: number;
  /** 敗着。盤に触らずに場所が分かるように赤い印を打つ */
  blunder?: { at: number; loss: number };
  onSeek: (n: number) => void;
  /** 送りの釦を帯に出すか。**呼ぶ側が既に送りの行を持っているときは false**
   *  — 棋譜ビューアは絵 (§9) が帯の上に 5 つ並べる形なので、そちらに任せる。
   *  既定で出すのは、帯だけ置くと 1 手ずつ動かせなくなるため。 */
  nav?: boolean;
}) {
  /* 送りの釦は**この帯の左端**。設計 §2 は 32px の帯に 24px の四角 4 つを
     溝 4px で並べ、ツールバーからは外している。**辿る道具を 1 か所に
     まとめる** — 帯で掴んで滑らせるのと 1 手ずつ送るのは同じ操作で、
     離して置くと ▶ を押した結果が別の場所 (帯の丸) に出る。 */
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

  const step = (n: number) => onSeek(Math.max(0, Math.min(plies, n)));

  return (
    <div style={{
      padding: '0 var(--sp-4)', flex: 'none',
      display: 'flex', alignItems: 'center', gap: 10,
    }}>
      {nav && (
        <span style={{ display: 'flex', gap: 4, flex: 'none' }}>
          <Button square size="row" title="最初へ" disabled={cursor === 0} onClick={() => step(0)}>|◀</Button>
          <Button square size="row" title="前の手" disabled={cursor === 0} onClick={() => step(cursor - 1)}>◀</Button>
          <Button square size="row" title="次の手" disabled={cursor >= plies} onClick={() => step(cursor + 1)}>▶</Button>
          <Button square size="row" title="最後へ" disabled={cursor >= plies} onClick={() => step(plies)}>▶|</Button>
        </span>
      )}
      <div ref={box} role="slider" aria-label="手数" aria-valuemin={0}
           aria-valuemax={plies} aria-valuenow={cursor} tabIndex={0}
           onPointerDown={(e) => {
             (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
             seekAt(e.clientX);
           }}
           onPointerMove={(e) => { if (e.buttons & 1) seekAt(e.clientX); }}
           style={{
             // 高さは 5 段のうちの --h-field。10 手ごとの数字は帯の外 (下の
             // 12px の行) に置くので、32px でも刻みと掴む丸 (16px) は収まる
             position: 'relative', height: 'var(--h-field)', flex: 1, minWidth: 0,
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
