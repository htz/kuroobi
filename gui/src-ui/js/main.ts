import { api } from './api';
import type { GameView } from './types';

const SVGNS = 'http://www.w3.org/2000/svg';
const CELL = 100, PAD = 40;
let view: GameView | null = null;
let hints: Record<number, { value: number; exact: boolean }> | null = null;
let thinking = false;   // エンジンの思考 (think) が進行中
// 一局を通した思考時間 (秒)。手番ごとに積む。
const thinkTotal = { black: 0, white: 0 };
let thinkTimer: number | null = null;
let playing = false;    // 対局中 (エンジンが自動で応手する状態)
let appMode: 'vs' | 'study' = 'vs';     // 'vs' = 対局モード, 'study' = 検討モード
let autoHint = false;
let hintSeq = 0;        // 解析要求の世代 (古い結果を捨てる)
// 手数 → その手の由来 ('book' | 'search')。エンジンが指した手だけ記録する。
let moveSource: Record<number, 'book' | 'search'> = {};

const $ = <T extends HTMLElement = HTMLElement>(id: string): T => document.getElementById(id) as T;

function el(name: string, attrs: Record<string, string | number>, text?: string | number): SVGElement {
  const e = document.createElementNS(SVGNS, name);
  for (const k in attrs) e.setAttribute(k, String(attrs[k]));
  if (text !== undefined) e.textContent = String(text);
  return e;
}

function fillSelect(id: string, from: number, to: number, selected: number, labelFn?: (v: number) => string): void {
  const s = $(id);
  for (let v = from; v <= to; v++) {
    const o = document.createElement('option');
    o.value = String(v);
    o.textContent = labelFn ? labelFn(v) : String(v);
    if (v === selected) o.selected = true;
    s.appendChild(o);
  }
}
// 深さ 60 = 初手から終局まで全読み相当。空き 36 超の完全読みや帯なしの
// 深さ 30 超は現実的な時間では返らないことがある。
fillSelect('depth', 1, 60, 12);
fillSelect('solve', 0, 36, 18, v => v === 0 ? 'なし' : String(v));
fillSelect('band', 0, 12, 0, v => v === 0 ? 'なし' : `+${v}`);

const LEVELS = [
  { name: 'Lv1 (入門)',  depth: 1,  solve: 2,  band: 0 },
  { name: 'Lv2',         depth: 2,  solve: 4,  band: 0 },
  { name: 'Lv3',         depth: 4,  solve: 8,  band: 0 },
  { name: 'Lv4',         depth: 6,  solve: 10, band: 0 },
  { name: 'Lv5',         depth: 8,  solve: 12, band: 0 },
  { name: 'Lv6',         depth: 10, solve: 14, band: 0 },
  { name: 'Lv7',         depth: 12, solve: 16, band: 0 },
  { name: 'Lv8',         depth: 14, solve: 18, band: 0 },
  { name: 'Lv9',         depth: 16, solve: 20, band: 0 },
  { name: 'Lv10',        depth: 18, solve: 22, band: 6 },
  { name: 'Lv11',        depth: 20, solve: 24, band: 6 },
  { name: 'Lv12 (GGS 設定)', depth: 22, solve: 26, band: 6 },
  { name: 'Lv13 (全力)', depth: 24, solve: 26, band: 8 },
];
{
  const s = $<HTMLSelectElement>('level');
  LEVELS.forEach((lv, i) => {
    const o = document.createElement('option');
    o.value = String(i);
    o.textContent = `${lv.name} — 深さ${lv.depth} / 読切${lv.solve}${lv.band ? ` / 帯+${lv.band}` : ''}`;
    if (i === 6) o.selected = true;
    s.appendChild(o);
  });
  const o = document.createElement('option');
  o.value = 'custom';
  o.textContent = 'カスタム…';
  s.appendChild(o);
}

function currentLevels(): { depth: number; solve: number; band: number } {
  const v = $<HTMLSelectElement>('level').value;
  if (v === 'custom') {
    return { depth: +$<HTMLSelectElement>('depth').value, solve: +$<HTMLSelectElement>('solve').value, band: +$<HTMLSelectElement>('band').value };
  }
  const lv = LEVELS[+v];
  return { depth: lv.depth, solve: lv.solve, band: lv.band };
}

// file-major: sq = file*8 + rank
function xy(sq: number): [number, number] { return [Math.floor(sq / 8), sq % 8]; }

function render() {
  const svg = $('board');
  svg.innerHTML = '';
  svg.appendChild(el('rect', { x: 0, y: 0, width: 880, height: 880, rx: 14, fill: '#20252c' }));
  svg.appendChild(el('rect', { x: PAD - 6, y: PAD - 6, width: 812, height: 812, rx: 6, fill: 'var(--board-dark)' }));
  svg.appendChild(el('rect', { x: PAD, y: PAD, width: 800, height: 800, fill: 'var(--board)' }));
  for (let i = 0; i <= 8; i++) {
    svg.appendChild(el('line', { x1: PAD + i * CELL, y1: PAD, x2: PAD + i * CELL, y2: PAD + 800, stroke: 'var(--line)', 'stroke-width': 2 }));
    svg.appendChild(el('line', { x1: PAD, y1: PAD + i * CELL, x2: PAD + 800, y2: PAD + i * CELL, stroke: 'var(--line)', 'stroke-width': 2 }));
  }
  for (let i = 0; i < 8; i++) {
    svg.appendChild(el('text', { x: PAD + i * CELL + 50, y: 27, 'text-anchor': 'middle', fill: 'var(--sub)', 'font-size': 19 }, 'abcdefgh'[i]));
    svg.appendChild(el('text', { x: 21, y: PAD + i * CELL + 57, 'text-anchor': 'middle', fill: 'var(--sub)', 'font-size': 19 }, String(i + 1)));
  }
  for (const [fx, fy] of [[2, 2], [2, 6], [6, 2], [6, 6]])
    svg.appendChild(el('circle', { cx: PAD + fx * CELL, cy: PAD + fy * CELL, r: 5, fill: 'var(--line)' }));

  if (!view) return;
  const legal = new Set(view.legal);
  const bestHint = hints ? Math.max(...Object.values(hints).map(x => x.value)) : null;
  for (let sq = 0; sq < 64; sq++) {
    const [f, r] = xy(sq);
    const cx = PAD + f * CELL + 50, cy = PAD + r * CELL + 50;
    const v = view.cells[sq];
    if (v === 1 || v === 2) {
      // フラットな円盤 (オセロ石)。薄い落ち影 + 細いリムのみ
      svg.appendChild(el('circle', { cx, cy: cy + 2, r: 40, fill: 'rgba(0,0,0,.25)' }));
      svg.appendChild(el('circle', {
        cx, cy, r: 40,
        fill: v === 1 ? '#111213' : '#f1f1f1',
        stroke: v === 1 ? '#2c2e30' : '#c9cdd1', 'stroke-width': 2,
      }));
      if (view.last === sq)
        svg.appendChild(el('circle', { cx, cy, r: 9, fill: 'none', stroke: 'var(--accent)', 'stroke-width': 4 }));
    } else if (legal.has(sq)) {
      const g = el('g', { cursor: thinking ? 'default' : 'pointer' });
      g.appendChild(el('circle', { cx, cy, r: 46, fill: 'transparent' }));
      // 棋譜上でこの局面の次に実際に指された手は金色のリングで区別する
      if (view.moves[view.cursor] === sq) {
        g.appendChild(el('circle', {
          cx, cy, r: 38, fill: 'none',
          stroke: 'var(--gold)', 'stroke-width': 2.5, 'stroke-dasharray': '6 5',
        }));
      }
      const h = hints ? hints[sq] : undefined;
      if (h) {
        const isBest = bestHint !== null && h.value >= bestHint - 1e-6;
        g.appendChild(el('circle', { cx, cy, r: 30, fill: isBest ? 'rgba(255,212,121,.14)' : 'rgba(255,255,255,.06)', stroke: isBest ? 'var(--gold)' : 'rgba(255,255,255,.25)', 'stroke-width': isBest ? 2 : 1 }));
        g.appendChild(el('text', {
          x: cx, y: cy + 8, 'text-anchor': 'middle', 'font-size': 24,
          fill: isBest ? 'var(--gold)' : '#d9e6de', 'font-weight': isBest ? 700 : 400,
        }, (h.value > 0 ? '+' : '') + h.value.toFixed(h.exact ? 0 : 1)));
      } else {
        const dot = el('circle', { cx, cy, r: 7, fill: 'rgba(255,255,255,.30)' });
        g.appendChild(dot);
        if (!thinking) {
          g.addEventListener('mouseenter', () => dot.setAttribute('r', '12'));
          g.addEventListener('mouseleave', () => dot.setAttribute('r', '7'));
        }
      }
      if (!thinking) g.addEventListener('click', () => humanPlay(sq));
      svg.appendChild(g);
    }
  }
}

function updatePanel(): void {
  if (!view) return;
  $('nb').textContent = String(view.black);
  $('nw').textContent = String(view.white);
  $('score-b').classList.toggle('turn', !view.over && view.player === 'black');
  $('score-w').classList.toggle('turn', !view.over && view.player === 'white');
  renderKifu();
  $('result').textContent = view.over
    ? (view.black === view.white ? '引き分け' : (view.black > view.white ? '黒の勝ち' : '白の勝ち'))
    : '';
  $<HTMLButtonElement>('btn-undo').disabled = thinking || view.move_count === 0;
  $<HTMLButtonElement>('btn-new').disabled = thinking;
  const start = $<HTMLButtonElement>('btn-start');
  start.textContent = playing ? '■ 対局停止' : '▶ 対局開始';
  start.classList.toggle('primary', !playing);
  start.disabled = !playing && (view.over || thinking);
  // モード別の表示: 対局 = 担当 + 開始/停止 + 新規/待った、検討 = 評価グラフ
  start.style.display = appMode === 'vs' ? '' : 'none';
  $('side-row').style.display = appMode === 'vs' ? '' : 'none';
  $('game-btns').style.display = appMode === 'vs' ? 'flex' : 'none';
  $('graph-card').style.display = appMode === 'study' ? 'flex' : 'none';
}

function setPlaying(b: boolean): void {
  playing = b;
  updatePanel();
}

const sqName = (sq: number): string => 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);

// ---- 評価値グラフ (黒視点の折れ線、1 系列) ----
let graphVals: ({ value: number; exact: boolean } | undefined)[] | null = null;   // [n] = {value, exact} / undefined。長さ moves.length+1
let graphKey = '';      // どの手順に対する値か (変わったら無効化)
let graphSeq = 0;
let graphBusy = false;
const lineKey = () => view ? view.moves.map(m => m == null ? 'p' : m).join(',') : '';

async function updateGraph() {
  if (graphBusy || !view) return;
  graphBusy = true;
  const seq = ++graphSeq;
  const key = lineKey();
  const len = view.moves.length;
  if (!graphVals || graphKey !== key) { graphVals = new Array(len + 1); graphKey = key; }
  await pushLevels();
  // グラフは全局面を測るので深さは控えめに (レベル深さ、上限 14)
  const depth = Math.min(currentLevels().depth, 14);
  for (let n = 0; n <= len; n++) {
    if (seq !== graphSeq || lineKey() !== key) break;
    if (graphVals[n]) continue;
    // 次の手がパス = 打てない手番の局面は測らない (0 が混ざるだけ)
    if (n < len && view.moves[n] == null) continue;
    setStatus(`グラフ計算中 ${n}/${len}…`, true);
    try {
      const p = await api.evalAt(n, depth);
      if (seq !== graphSeq || lineKey() !== key) break;
      if (Number.isFinite(p.value)) graphVals[n] = { value: p.value, exact: p.exact };
    } catch (e) { setStatus('' + e, false); break; }
    drawGraph();
  }
  if (seq === graphSeq) setStatus('', false);
  graphBusy = false;
}

function drawGraph() {
  const svg = $('graph');
  svg.innerHTML = '';
  const W = 800, H = 190, L = 36, R = 16, T = 16, B = 24;
  svg.appendChild(el('rect', { x: 0, y: 0, width: W, height: H, rx: 8, fill: '#1a1f25' }));
  if (!view) return;
  const len = Math.max(1, view.moves.length);
  const vals = graphVals || [];
  const defined = vals.filter((v): v is { value: number; exact: boolean } => !!v).map(v => Math.abs(v.value));
  const ymax = Math.max(8, Math.min(64, Math.ceil((defined.length ? Math.max(...defined) : 0) / 8) * 8 || 8));
  const clampV = (v: number): number => Math.max(-ymax, Math.min(ymax, v));
  const x = (n: number): number => L + (W - L - R) * n / len;
  const y = (v: number): number => T + (H - T - B) * (1 - (v + ymax) / (2 * ymax));

  // 目盛 (控えめ): ±ymax とゼロ線
  const grid = (v: number, label: string, strong: boolean): void => {
    svg.appendChild(el('line', { x1: L, y1: y(v), x2: W - R, y2: y(v), stroke: strong ? '#3a4450' : '#272e37', 'stroke-width': 1 }));
    svg.appendChild(el('text', { x: L - 4, y: y(v) + 3, 'text-anchor': 'end', fill: 'var(--sub)', 'font-size': 11 }, label));
  };
  grid(ymax, '+' + ymax, false);
  grid(-ymax, '-' + ymax, false);
  grid(0, '0', true);
  svg.appendChild(el('text', { x: W - R, y: T + 7, 'text-anchor': 'end', fill: 'var(--sub)', 'font-size': 11 }, '黒有利'));
  svg.appendChild(el('text', { x: W - R, y: H - B - 2, 'text-anchor': 'end', fill: 'var(--sub)', 'font-size': 11 }, '白有利'));
  for (let n = 10; n <= view.moves.length; n += 10)
    svg.appendChild(el('text', { x: x(n), y: H - 4, 'text-anchor': 'middle', fill: 'var(--sub)', 'font-size': 11 }, String(n)));

  // 現在位置
  svg.appendChild(el('line', { x1: x(view.cursor), y1: T, x2: x(view.cursor), y2: H - B, stroke: 'var(--accent-dim)', 'stroke-width': 1, 'stroke-dasharray': '3 3' }));

  if (!graphVals || defined.length === 0) {
    svg.appendChild(el('text', { x: (L + W - R) / 2, y: H / 2 - 8, 'text-anchor': 'middle', fill: 'var(--sub)', 'font-size': 13 },
      graphBusy ? '計算中…' : '「更新」で全局面を採点します'));
    return;
  }

  // 折れ線 (パス・未計算の点は飛ばして繋ぐ)
  let d = '', pen = false;
  vals.forEach((v: { value: number; exact: boolean } | undefined, n: number) => {
    if (!v) return;
    d += (pen ? 'L' : 'M') + x(n).toFixed(1) + ' ' + y(clampV(v.value)).toFixed(1) + ' ';
    pen = true;
  });
  svg.appendChild(el('path', { d, fill: 'none', stroke: 'var(--accent)', 'stroke-width': 2, 'stroke-linejoin': 'round' }));
  vals.forEach((v: { value: number; exact: boolean } | undefined, n: number) => {
    if (!v) return;
    svg.appendChild(el('circle', {
      cx: x(n), cy: y(clampV(v.value)), r: v.exact ? 4 : 3,
      fill: v.exact ? 'var(--gold)' : 'var(--accent)',
      stroke: '#1a1f25', 'stroke-width': 1,
    }));
  });

  // ホバー: 最寄り点の値を表示、クリックでその局面へ
  const tipDot = el('circle', { r: 6, fill: 'none', stroke: '#fff', 'stroke-width': 1.5, visibility: 'hidden' });
  const tipText = el('text', { 'font-size': 12, fill: 'var(--text)', 'text-anchor': 'middle', visibility: 'hidden' });
  svg.appendChild(tipDot); svg.appendChild(tipText);
  const hover = el('rect', { x: L, y: 0, width: W - L - R, height: H, fill: 'transparent', cursor: 'pointer' });
  const nearest = (ev: MouseEvent): number | null => {
    const r = svg.getBoundingClientRect();
    const mx = (ev.clientX - r.left) * (W / r.width);
    let n = Math.round((mx - L) / ((W - L - R) / len));
    n = Math.max(0, Math.min(view!.moves.length, n));
    return vals[n] ? n : null;
  };
  hover.addEventListener('mousemove', (ev) => {
    const n = nearest(ev as MouseEvent);
    const v = n === null ? undefined : vals[n];
    if (n === null || !v) {
      tipDot.setAttribute('visibility', 'hidden');
      tipText.setAttribute('visibility', 'hidden');
      return;
    }
    tipDot.setAttribute('cx', String(x(n)));
    tipDot.setAttribute('cy', String(y(clampV(v.value))));
    tipDot.setAttribute('visibility', 'visible');
    tipText.textContent = `${n}手 ${v.value > 0 ? '+' : ''}${v.exact ? v.value.toFixed(0) : v.value.toFixed(1)}`;
    tipText.setAttribute('x', String(Math.max(L + 20, Math.min(W - R - 20, x(n)))));
    tipText.setAttribute('y', String(y(clampV(v.value)) < H / 2 ? y(clampV(v.value)) + 14 : y(clampV(v.value)) - 8));
    tipText.setAttribute('visibility', 'visible');
  });
  hover.addEventListener('mouseleave', () => {
    tipDot.setAttribute('visibility', 'hidden');
    tipText.setAttribute('visibility', 'hidden');
  });
  hover.addEventListener('click', (ev) => {
    const n0 = nearest(ev as MouseEvent);
    if (n0 === null || !view) return;
    let n = n0;
    while (n < view.moves.length && view.moves[n] == null) n++;
    void jumpTo(n);
  });
  svg.appendChild(hover);
}

// 棋譜をチップ列で描く。クリックで**その手を打った直後**の局面へ移動。
// 先の手は薄く残り、前へも戻れる。
function renderKifu(): void {
  const v0 = view;
  if (!v0) return;
  const box = $('kifu');
  box.innerHTML = '';
  v0.moves.forEach((m: number | null, i: number) => {
    const n = i + 1;                         // n 手目
    const d = document.createElement('span');
    let cls = 'mv';
    if (n === v0.cursor) cls += ' current';   // 最後に打たれた手
    if (n > v0.cursor) cls += ' future';
    if (m == null) cls += ' pass';
    const src = moveSource[n];
    if (src) cls += ' src-' + src;
    d.className = cls;
    if (src) d.title = src === 'book' ? '定石どおりの手' : 'エンジンの探索による手';
    const num = document.createElement('i');
    num.textContent = String(n);
    d.appendChild(num);
    // パス込みで 1 手ごとに手番が入れ替わるので、奇数手 = 黒で確定
    const st = document.createElement('span');
    st.className = 'st ' + (i % 2 === 0 ? 'b' : 'w');
    d.appendChild(st);
    d.appendChild(document.createTextNode(m == null ? 'ps' : sqName(m)));
    d.addEventListener('click', () => {
      // 着地先の次の手がパスなら「打てない手番」で止まらないよう、
      // パスを消化した局面まで進めて着地する
      let n = i + 1;
      while (n < v0.moves.length && v0.moves[n] == null) n++;
      void jumpTo(n);
    });
    box.appendChild(d);
  });
  const cur = box.querySelector('.current');
  if (cur) cur.scrollIntoView({ block: 'nearest' });
}

async function jumpTo(n: number): Promise<void> {
  if (thinking) return;
  hintSeq++;
  api.stopSearch().catch(() => {});   // 進行中の解析を打ち切る
  if (playing) {
    setPlaying(false);
    setStatus('棋譜を移動したため停止しました', false);
  }
  try {
    hints = null;
    setView(await api.goto(n));
  } catch (e) { setStatus('' + e, false); }
}

function setView(v: GameView): void {
  view = v;
  if (graphKey !== lineKey()) { graphVals = null; graphKey = lineKey(); }
  render(); updatePanel(); drawGraph(); refreshHints();
}

// 常時ヒント: 人の手番 (または検討モード) になったら裏で採点する。
// 盤操作はブロックせず、返ってきた時に局面が進んでいたら捨てる。
async function refreshHints() {
  if (!autoHint || !view || view.over || thinking) return;
  if (playing && engineSide().includes(view.player)) return; // エンジンが打つ局面は不要
  const seq = ++hintSeq;
  const kifuAt = view.kifu;
  setStatus('解析中…', true);
  try {
    await pushLevels();
    const hs = await api.analyze(currentLevels().depth);
    if (seq !== hintSeq || !view || view.kifu !== kifuAt) return;  // 古い
    hints = {};
    for (const h of hs) {
      if (typeof h.value !== 'number' || !Number.isFinite(h.value)) continue;
      hints[h.pos] = { value: h.value, exact: h.exact };
    }
    if (Object.keys(hints).length === 0) hints = null;
    render();
  } catch (e) {
    if (seq === hintSeq) setStatus('' + e, false);
    return;
  }
  if (seq === hintSeq) setStatus('', false);
}
function setStatus(s: string, spin?: boolean): void {
  $('status').textContent = s;
  $('status').className = spin ? 'spin' : '';
}

const fmtSecs = (v: number): string =>
  v >= 60 ? `${Math.floor(v / 60)} 分 ${(v % 60).toFixed(0)} 秒` : `${v.toFixed(1)} 秒`;

/// 思考時間の合計を出す。まだ誰も考えていなければ隠す。
function renderThinkTime(): void {
  const box = $('think-time');
  const any = thinkTotal.black > 0 || thinkTotal.white > 0;
  box.hidden = !any;
  $('tb').textContent = fmtSecs(thinkTotal.black);
  $('tw').textContent = fmtSecs(thinkTotal.white);
}

/// 思考中は経過時間を出し続ける (どれだけ待たされているかが分かる)。
function startThinkClock(): void {
  const t0 = performance.now();
  stopThinkClock();
  const tick = () => {
    setStatus(`思考中… ${((performance.now() - t0) / 1000).toFixed(1)} 秒`, true);
  };
  tick();
  thinkTimer = window.setInterval(tick, 100);
}

function stopThinkClock(): void {
  if (thinkTimer !== null) { clearInterval(thinkTimer); thinkTimer = null; }
}

function engineSide(): string[] {
  if (appMode === 'study') return [];   // 検討モードではエンジンは打たない
  const m = document.querySelector<HTMLElement>('#mode button.active')?.dataset.v ?? 'off';
  if (m === 'both') return ['black', 'white'];
  if (m === 'off') return [];           // 人が両方を打つ
  return [m];
}

async function pushLevels() {
  const { depth, solve, band } = currentLevels();
  await api.setLevels(depth, solve, band).catch(() => {});
}

/* ---- エンジンが使うファイル ---- */
const RES_KINDS: [string, string][] = [
  ['dir', '置き場所 (フォルダ)'],
  ['weights', '線形評価の重み'],
  ['nnue', 'NNUE の重み'],
  ['book', '定石のファイル'],
];

async function renderFiles(): Promise<void> {
  const box = $('files-list');
  box.textContent = '';
  const st = await api.resourceStatus();
  // status は weights / nnue / book の 3 つ。フォルダは個別に足す。
  const byName = new Map(st.map(([n, p, ok]) => [n, { p, ok }]));
  const label: Record<string, string> = {
    weights: '線形評価の重み', nnue: 'NNUE の重み', book: '定石のファイル',
  };
  for (const [kind, title] of RES_KINDS) {
    const info = kind === 'dir' ? null : byName.get(label[kind]);
    const row = document.createElement('div');
    row.className = 'file-row';
    const head = document.createElement('div');
    head.className = 'file-head';
    head.textContent = title;
    if (info) {
      const badge = document.createElement('span');
      badge.className = 'file-state ' + (info.ok ? 'ok' : 'ng');
      badge.textContent = info.ok ? '見つかりました' : '見つかりません';
      head.append(badge);
    }
    const path = document.createElement('div');
    path.className = 'file-path';
    path.textContent = info ? info.p : '(未指定なら自動で探します)';
    const btns = document.createElement('div');
    btns.className = 'row actions';
    const pick = document.createElement('button');
    pick.className = 'btn small';
    pick.textContent = '選ぶ…';
    pick.onclick = async () => {
      const p = await api.pickResource(kind);
      if (!p) return;
      await api.setResource(kind, p);
      await renderFiles();
      await syncBookAvailability();
      setStatus('使うファイルを変更しました (次の思考から反映)', false);
    };
    const clear = document.createElement('button');
    clear.className = 'btn small ghost';
    clear.textContent = '既定に戻す';
    clear.onclick = async () => {
      await api.setResource(kind, null);
      await renderFiles();
      await syncBookAvailability();
    };
    btns.append(pick, clear);
    row.append(head, path, btns);
    box.append(row);
  }
}

$('btn-files').addEventListener('click', async () => {
  $('files-modal').hidden = false;
  await renderFiles();
});
$('files-close').addEventListener('click', () => { $('files-modal').hidden = true; });

// 定石の on/off。切ると序盤から自力で読む (研究向け)。
document.querySelectorAll<HTMLElement>('#use-book button').forEach((b) => {
  b.addEventListener('click', async () => {
    if (b.classList.contains('disabled')) return;
    document.querySelectorAll('#use-book button').forEach((x) => x.classList.remove('active'));
    b.classList.add('active');
    await api.setUseBook(b.dataset.v === 'on')
      .catch((e) => setStatus('book 切り替え失敗: ' + e, false));
    hints = null;
    refreshHints();
  });
});

/// book のファイルが無ければ選べない。設定から選び直せることも伝える。
async function syncBookAvailability(): Promise<void> {
  let ok = false;
  try { ok = await api.hasBook(); } catch { /* エンジン未初期化 */ }
  const seg = $('use-book');
  seg.classList.toggle('disabled', !ok);
  document.querySelectorAll<HTMLElement>('#use-book button').forEach((b) => {
    b.classList.toggle('disabled', !ok);
    if (!ok) b.classList.toggle('active', b.dataset.v === 'off');
  });
  $('book-note').textContent = ok ? '' : '— ファイルがありません (歯車から指定できます)';
}
void syncBookAvailability();

async function maybeEngineTurn() {
  while (playing && view && !view.over && engineSide().includes(view.player)) {
    thinking = true;
    const side = view.player as 'black' | 'white';
    startThinkClock();
    render(); updatePanel();
    let r;
    try {
      r = await api.think();
    } catch (e) {
      thinking = false;
      stopThinkClock();
      // 停止済みで局面が変わっていた場合はエラー扱いにしない
      if (playing) { setStatus('エンジンエラー: ' + e, false); setPlaying(false); }
      render(); updatePanel();
      return;
    }
    thinking = false;
    stopThinkClock();
    thinkTotal[side] += r.secs;
    renderThinkTime();
    if (!playing) {
      // 思考中に停止された: 結果は適用せず捨てる
      setStatus('対局を停止しました', false);
      render(); updatePanel();
      refreshHints();
      return;
    }
    try {
      const v = await api.applyMove(r.pos);
      hints = null;
      // この手が book 由来か探索由来かを棋譜に残す (v.cursor が指した後の手数)
      moveSource[v.cursor] = r.from_book ? 'book' : 'search';
      setView(v);
      $('eval').textContent = Number.isFinite(r.value)
        ? `エンジン評価: ${r.value > 0 ? '+' : ''}${r.exact ? r.value.toFixed(0) : r.value.toFixed(1)} 石` +
          (r.exact ? ' (完全読み)' : (r.from_book ? ' (定石)' : ''))
        : '';
    } catch (e) {
      setStatus('' + e, false);
      setPlaying(false);
      return;
    }
    setStatus('', false);
    updatePanel();
  }
  if (view && view.over) setPlaying(false);
  refreshHints();
}

async function humanPlay(sq: number): Promise<void> {
  if (thinking) return;
  if (playing && view && engineSide().includes(view.player)) return;  // エンジンの手番
  try {
    const v = await api.play(sq);
    hints = null;
    setView(v);
    if (playing) await maybeEngineTurn();
  } catch (e) { setStatus('' + e, false); }
}

$<HTMLButtonElement>('btn-start').addEventListener('click', async () => {
  if (playing) {
    // 停止: エンジンの探索も実際に中断させる (結果は破棄)
    setPlaying(false);
    api.stopSearch().catch(() => {});
    setStatus('対局を停止しました', false);
    return;
  }
  await pushLevels();
  graphSeq++;               // 進行中のグラフ計算を打ち切る
  setPlaying(true);
  setStatus('', false);
  await maybeEngineTurn();
});

$<HTMLButtonElement>('btn-new').addEventListener('click', async () => {
  await pushLevels();
  hints = null;
  moveSource = {};
  thinkTotal.black = 0;
  thinkTotal.white = 0;
  renderThinkTime();
  $('eval').textContent = '';
  setPlaying(false);
  setView(await api.newGame());
  setStatus('「▶ 対局開始」で対局、そのまま打てば検討モード', false);
});

$<HTMLButtonElement>('btn-undo').addEventListener('click', async () => {
  try {
    hints = null;
    setView(await api.undo());
    if (view && engineSide().includes(view.player) && view.move_count > 0)
      setView(await api.undo());
  } catch (e) { setStatus('' + e, false); }
});

document.querySelectorAll<HTMLElement>('#app-mode button').forEach(b => {
  b.addEventListener('click', () => {
    document.querySelectorAll('#app-mode button').forEach(x => x.classList.remove('active'));
    b.classList.add('active');
    appMode = b.dataset.v === 'study' ? 'study' : 'vs';
    if (appMode === 'study' && playing) setPlaying(false);
    graphSeq++;             // モードをまたぐグラフ計算は打ち切る
    setStatus(appMode === 'study'
      ? '検討モード: 両方の手を自由に打てます'
      : '対局モード: 「▶ 対局開始」でエンジンと対局', false);
    updatePanel(); drawGraph(); refreshHints();
  });
});

document.querySelectorAll<HTMLElement>('#mode button').forEach(b => {
  b.addEventListener('click', () => {
    document.querySelectorAll('#mode button').forEach(x => x.classList.remove('active'));
    b.classList.add('active');
    // 担当を変えたら停止状態に戻す (再開は「対局開始」で明示的に)
    if (playing) {
      setPlaying(false);
      setStatus('担当を変更したため停止しました', false);
    }
  });
});

document.querySelectorAll<HTMLElement>('#auto-hint button').forEach(b => {
  b.addEventListener('click', () => {
    document.querySelectorAll('#auto-hint button').forEach(x => x.classList.remove('active'));
    b.classList.add('active');
    autoHint = b.dataset.v === 'on';
    if (autoHint) {
      refreshHints();
    } else {
      hintSeq++;          // 進行中の解析結果を無効化
      api.stopSearch().catch(() => {});
      hints = null;
      render();
    }
  });
});

$('btn-graph').addEventListener('click', updateGraph);

$('btn-save').addEventListener('click', async () => {
  try {
    const p = await api.saveKifu();
    if (p) setStatus('保存しました: ' + p, false);
  } catch (e) { setStatus('' + e, false); }
});

function afterKifuLoad(v: GameView): void {
  setPlaying(false);
  hints = null;
  moveSource = {};
  $('eval').textContent = '';
  $('paste-modal').hidden = true;
  setView(v);
  setStatus('棋譜を読み込みました', false);
}

$('btn-load').addEventListener('click', () => {
  $<HTMLTextAreaElement>('paste-text').value = '';
  $('paste-modal').hidden = false;
  $<HTMLTextAreaElement>('paste-text').focus();
});

$('paste-cancel').addEventListener('click', () => { $('paste-modal').hidden = true; });

$('paste-ok').addEventListener('click', async () => {
  try {
    const text = $<HTMLTextAreaElement>('paste-text').value;
    if (!text.trim()) return;
    afterKifuLoad(await api.loadKifuText(text));
  } catch (e) { setStatus('' + e, false); $('paste-modal').hidden = true; }
});

$('paste-file').addEventListener('click', async () => {
  try {
    const v = await api.loadKifu();
    if (v) afterKifuLoad(v);
  } catch (e) { setStatus('' + e, false); $('paste-modal').hidden = true; }
});

$<HTMLSelectElement>('level').addEventListener('change', () => {
  $('custom-row').style.display = $<HTMLSelectElement>('level').value === 'custom' ? 'flex' : 'none';
  pushLevels();
});
$<HTMLSelectElement>('depth').addEventListener('change', pushLevels);
$<HTMLSelectElement>('solve').addEventListener('change', pushLevels);
$<HTMLSelectElement>('band').addEventListener('change', pushLevels);

(async () => {
  render();
  try {
    setView(await api.state());
    await pushLevels();
  } catch (e) {
    setStatus('初期化エラー: ' + e, false);
  }
})();
