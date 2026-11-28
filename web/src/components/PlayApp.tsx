import React from 'react';
import { Engine } from '../engine/client';
import type { Valued } from '../engine/protocol';
import { href, t, type Lang } from '../i18n';
import { isOver, legal, play, replay, squareName, START, type Position, type Side } from '../lib/othello';
import { boardText, isStart, readRecord, type GameRecord } from '../lib/record';
import { Board, BoardDefs, EvalLegend, FLIP_MS, type EvalInfo } from './Board';
import { EvalGraph, type GraphPoint } from './EvalGraph';
import { LoadRecord } from './LoadRecord';
import {
  Busy, Button, BoardCover, CandidateList, KifuTable, LEVEL0, levelLabel, MenuButton, Panel, Progress, RadioCards, Scoreboard, Segmented, StepNav, Strength, Toast, Toggle,
  type Levels, type Move, type Reading, type ScoreSide,
} from './ui';

/** The graph's search stops at this depth whatever the level, as the app's does. */
const GRAPH_DEPTH_MAX = 14;
/** A game is at most 60 moves; the graph's axis always spans them all. */
const PLIES = 60;
/** KUROOBI's stone lands no sooner than this after yours, so yours has turned its stones first however fast it reads. */
const REPLY_MIN_MS = 800;
/** After KUROOBI's stone lands, its turned stones settle before the turn comes back. */
const SETTLE_MS = FLIP_MS + 360;

type Mode = 'play' | 'study';
type Hints = { at: number; depth: number; moves: Valued[] };

const OPENING: Position = { cells: START.slice(), turn: 1 };

function line(start: Position, moves: number[]): Position[] {
  const out: Position[] = [start];
  for (const m of moves) out.push(play(out[out.length - 1], m)!);
  return out;
}

const count = (p: Position) => ({ b: p.cells.filter((c) => c === 1).length, w: p.cells.filter((c) => c === 2).length });
const empties = (p: Position) => p.cells.filter((c) => c === 0).length;

const kifuOf = (moves: number[]) => moves.map(squareName).join('');

/** The engine, made once in the browser; the server render never starts a worker. */
function useEngine(enabled: boolean) {
  const [engine, setEngine] = React.useState<Engine | null>(null);
  const [progress, setProgress] = React.useState<{ loaded: number; total: number } | null>(null);
  const [ready, setReady] = React.useState(false);
  const [failed, setFailed] = React.useState<string | null>(null);
  React.useEffect(() => {
    if (!enabled) return;
    const e = new Engine();
    e.onProgress = (loaded, total) => setProgress({ loaded, total });
    e.ready.then(() => setReady(true), (err: Error) => setFailed(err.message));
    setEngine(e);
  }, [enabled]);
  return { engine, progress, ready, failed };
}

const SAVED_GAME = 'kuroobi-play-game';
type SavedGame = { moves: number[]; game: { you: Side; level: Levels }; reads?: (Reading | null)[] };

/** `preview` draws a game in progress for the top page, with no engine behind it. */
export default function PlayApp({ lang, mode, preview }: { lang: Lang; mode: Mode; preview?: string }) {
  const L = (key: string, vars?: Record<string, string | number>) => t(lang, 'play.' + key, vars);
  const live = useEngine(!preview);
  const { engine, progress, failed } = live;
  const ready = live.ready || !!preview;

  const [moves, setMoves] = React.useState<number[]>(() => (preview ? replay(preview).moves : []));
  const [cursor, setCursor] = React.useState(0);
  // Study can start from any board a record gives; a game always starts from the opening.
  const [start, setStart] = React.useState<Position>(OPENING);
  const positions = React.useMemo(() => line(start, moves), [start, moves]);
  const at = mode === 'play' ? moves.length : Math.min(cursor, moves.length);
  const pos = positions[at];
  const over = isOver(pos);

  // Play
  const [setup, setSetup] = React.useState<{ you: Side; level: Levels }>({ you: 1, level: LEVEL0 });
  const [game, setGame] = React.useState(setup);
  // A game begins with its button; until then its colour and strength can change, and after it they cannot.
  const [playing, setPlaying] = React.useState(!!preview);
  /** The setup is over the board: at first, and after New game; Cancel always closes it on the board as it stands. */
  const [choosing, setChoosing] = React.useState(!preview);
  const [thinkingSince, setThinkingSince] = React.useState<number | null>(preview ? 0 : null);
  const thinking = thinkingSince !== null;
  // What KUROOBI read for each move it played, by ply; shown only when asked for.
  const [reads, setReads] = React.useState<(Reading | undefined)[]>([]);
  const [settling, setSettling] = React.useState(false);
  const [hintsOn, setHintsOn] = React.useState(false);
  const gen = React.useRef(0);

  // Study
  const [studyLevel, setStudyLevel] = React.useState<Levels>(LEVEL0);
  const [graph, setGraph] = React.useState<(GraphPoint | undefined)[]>([]);
  const [whole, setWhole] = React.useState<{ done: number; total: number } | null>(null);
  const wgen = React.useRef(0);

  // The live evals on the board, deepened a ply at a time until the position changes.
  const [hints, setHints] = React.useState<Hints | null>(null);
  const hgen = React.useRef(0);
  const hintsBusy = React.useRef(false);

  const [loadOpen, setLoadOpen] = React.useState(false);
  const [note, setNote] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (mode !== 'study') return;
    const q = new URLSearchParams(location.search);
    const given = q.get('start');
    const from = (given && readRecord(given)?.start) || OPENING;
    const ms = readRecord(`${boardText(from)} ${q.get('kifu') ?? ''}`)?.moves ?? [];
    const ply = Number(q.get('ply'));
    setStart(from);
    setMoves(ms);
    setCursor(Number.isInteger(ply) && q.has('ply') ? Math.min(Math.max(ply, 0), ms.length) : ms.length);
  }, [mode]);

  // A game in progress lives in the tab, so leaving for study (another page) and coming back finds it again.
  React.useEffect(() => {
    if (mode !== 'play' || preview) return;
    try {
      const saved = JSON.parse(sessionStorage.getItem(SAVED_GAME) ?? 'null') as SavedGame | null;
      if (!saved || !Array.isArray(saved.moves) || !line(OPENING, saved.moves).every(Boolean)) return;
      setSetup(saved.game);
      setGame(saved.game);
      setMoves(saved.moves);
      setReads((saved.reads ?? []).map((r) => r ?? undefined));
      setPlaying(true);
      setChoosing(false);
    } catch {
      // Without storage, or with something else in it, the page starts as new.
    }
  }, [mode, preview]);
  React.useEffect(() => {
    if (mode !== 'play' || preview) return;
    try {
      sessionStorage.setItem(SAVED_GAME, JSON.stringify(playing ? { moves, game, reads } : null));
    } catch {
      // The game still plays; it just will not be there after leaving the page.
    }
  }, [mode, preview, playing, moves, game, reads]);

  /** A reading still running holds the worker; whatever comes next must not wait behind it. */
  const dropHints = () => {
    hgen.current++;
    setHints(null);
    if (hintsBusy.current) {
      hintsBusy.current = false;
      engine?.restart();
    }
  };

  // KUROOBI moves whenever it is its turn in a game.
  React.useEffect(() => {
    if (mode !== 'play' || !playing || !engine || !ready) return;
    const p = positions[positions.length - 1];
    if (isOver(p) || p.turn === game.you) return;
    const g = ++gen.current;
    setThinkingSince(performance.now());
    const ply = positions.length - 1;
    const t0 = performance.now();
    engine.choose(p.cells, p.turn, game.level)
      .then((c) => new Promise<typeof c>((done) => setTimeout(() => done(c), Math.max(0, REPLY_MIN_MS - (performance.now() - t0)))))
      .then((c) => {
        if (g !== gen.current) return;
        setThinkingSince(null);
        if (c.move < 0) return;
        setReads((r) => { const x = r.slice(0, ply); x[ply] = { score: c.score, depth: c.depth, exact: c.exact, selective: c.selective || undefined, ms: c.ms, nodes: c.nodes }; return x; });
        setMoves((ms) => [...ms, c.move]);
        setSettling(true);
        setTimeout(() => { if (g === gen.current) setSettling(false); }, SETTLE_MS);
      });
  }, [mode, playing, engine, ready, positions, game]);

  const wantHints = !!engine && ready && !over && !whole && (mode === 'study' || (hintsOn && playing && !thinking && !settling && pos.turn === game.you));
  React.useEffect(() => {
    if (!wantHints || !engine) return;
    const g = ++hgen.current;
    const timer = setTimeout(() => {
      void (async () => {
        const full = empties(pos) - 1;
        hintsBusy.current = true;
        // As the engine's analyze_deepening (first_hint_depth, next_hint_depth): two plies a pass, ending at the
        // parity the game's search ends at, then the solve once it is within five more.
        let d = (64 - empties(pos)) % 2 === 0 ? 1 : 2;
        for (;;) {
          const ms = await engine.analyze(pos.cells, pos.turn, d);
          if (g !== hgen.current) return;
          setHints({ at, depth: d, moves: ms });
          if (d >= full || d >= PLIES) break;
          d = d + 2 < full - 5 ? d + 2 : Math.max(full, d + 1);
        }
        hintsBusy.current = false;
      })();
    }, 120);
    return () => clearTimeout(timer);
  }, [wantHints, engine, pos, at]);

  /** Begins a game with the settings chosen. */
  const begin = () => {
    gen.current++;
    dropHints();
    if (thinking) engine?.restart();
    else engine?.newGame();
    setGame(setup);
    setThinkingSince(null);
    setSettling(false);
    setReads([]);
    setMoves([]);
    setChoosing(false);
    setPlaying(true);
  };

  const undo = () => {
    dropHints();
    gen.current++;
    setSettling(false);
    if (thinking) {
      gen.current++;
      engine?.restart();
      setThinkingSince(null);
    }
    for (let i = moves.length - 1; i >= 0; i--) {
      if (positions[i].turn === game.you) {
        setMoves(moves.slice(0, i));
        setReads((r) => r.slice(0, i));
        return;
      }
    }
  };

  const jump = (n: number) => {
    if (n === at) return;
    dropHints();
    setCursor(n);
  };

  const onPlay = (sq: number) => {
    dropHints();
    if (mode === 'play') {
      setMoves([...moves, sq]);
      return;
    }
    setMoves([...moves.slice(0, at), sq]);
    setGraph((v) => v.slice(0, at + 1));
    setCursor(at + 1);
  };

  const stopAll = () => {
    wgen.current++;
    engine?.restart();
    setWhole(null);
  };

  // As the app does: from the last move back, each position valued once.
  const analyzeAll = async () => {
    if (!engine || whole) return;
    dropHints();
    const g = ++wgen.current;
    const ps = positions;
    const level = { depth: Math.min(studyLevel.depth, GRAPH_DEPTH_MAX), solve: studyLevel.solve };
    const out: (GraphPoint | undefined)[] = new Array(ps.length);
    setGraph(out.slice());
    let done = 0;
    for (let n = ps.length - 1; n >= 0; n--) {
      setWhole({ done, total: ps.length });
      const p = ps[n];
      if (isOver(p)) {
        const c = count(p);
        out[n] = { value: c.b - c.w, exact: true };
      } else {
        const v = await engine.evaluate(p.cells, p.turn, level);
        if (g !== wgen.current) return;
        out[n] = { value: (p.turn === 1 ? 1 : -1) * v.score, exact: v.exact, depth: v.depth, selective: v.selective || undefined };
      }
      done++;
      setGraph(out.slice());
    }
    setWhole(null);
  };

  const flash = (text: string) => {
    setNote(text);
    setTimeout(() => setNote(null), 2000);
  };
  const copy = async (text: string, done: string) => {
    try {
      await navigator.clipboard.writeText(text);
      flash(done);
    } catch {
      // Without clipboard access the text stays where it is shown.
    }
  };
  const studyUrl = (from: Position, ms: number[], ply?: number) => {
    const q = new URLSearchParams();
    if (!isStart(from)) q.set('start', boardText(from));
    if (ms.length) q.set('kifu', kifuOf(ms));
    if (ms.length && ply !== undefined && ply < ms.length) q.set('ply', String(ply));
    const qs = q.toString();
    return href(lang, '/play/study/') + (qs ? '?' + qs : '');
  };
  /** The record as the app writes it: the moves alone from the opening, the board before them otherwise. */
  const recordText = () => (isStart(start) ? kifuOf(moves) : `${boardText(start)} ${kifuOf(moves)}`.trim());

  const applyLoad = (r: GameRecord) => {
    if (mode === 'play') {
      location.href = studyUrl(r.start, r.moves);
      return;
    }
    dropHints();
    setStart(r.start);
    setMoves(r.moves);
    setGraph([]);
    setCursor(r.moves.length);
    setLoadOpen(false);
  };


  const c = count(pos);
  const yours = mode === 'play' && playing && !choosing && !over && pos.turn === game.you && !thinking && !settling;

  // Every eval on screen is a whole number; which move is best, and their order, come from the unrounded values.
  const showEvals = (on: boolean) => { if (!on) dropHints(); setHintsOn(on); };
  // The game's buttons: in its panel, or in one row under the board on a narrow screen (site.css)
  const gameButtons = mode === 'play' && !choosing;
  const undoButton = (
    <Button disabled={!playing || !moves.some((_, i) => positions[i].turn === game.you)} onClick={undo} style={{ flex: 1 }}>{L('undo')}</Button>
  );
  // With no game, or once it is over, the next one is what is left to do
  const newGameButton = (
    <Button kind={over || !playing ? 'primary' : 'secondary'} onClick={() => setChoosing(true)} style={{ flex: 1 }}>{L('newGame')}</Button>
  );
  const viewItems = [
    { label: L('showEvals'), checked: hintsOn, onSelect: () => showEvals(!hintsOn) },
  ];
  const shown = hints?.at === at && (mode === 'study' || hintsOn) ? hints : null;
  const exact = !!shown && empties(pos) - 1 <= shown.depth;
  const readNote = shown ? (exact ? L('solved') : L('depth', { n: shown.depth })) : '';
  const boardEvals: Record<number, EvalInfo> = Object.fromEntries(
    (shown?.moves ?? []).map((m, i) => [m.move, { score: m.score, best: i === 0, src: exact ? 'solve' as const : undefined }]),
  );

  // Each move's value after it from Black's view, and what it cost the side that played it (2 discs or more).
  const rows: Move[] = moves.map((m, i) => {
    const value = graph[i + 1]?.value;
    const prev = graph[i]?.value;
    const mover = positions[i].turn;
    const drop = value === undefined || prev === undefined ? 0 : mover === 1 ? prev - value : value - prev;
    const read = mode === 'play' && mover !== game.you ? reads[i] : undefined;
    const after = graph[i + 1];
    return {
      n: i + 1, move: squareName(m), color: mover === 1 ? 'b' : 'w', score: value, loss: drop >= 2 ? drop : undefined,
      depth: after && !after.exact ? after.depth : undefined, solved: after?.exact, selective: after?.selective, read,
    };
  });
  const blunder = rows.reduce<{ at: number; loss: number } | undefined>(
    (b, r) => (r.loss && (!b || r.loss > b.loss) ? { at: r.n, loss: r.loss } : b), undefined);

  const kuroobi: Side = (3 - game.you) as Side;
  // The side to move stands out; until KUROOBI's stones settle the turn stays with it. KUROOBI's card says only that it thinks.
  const going = (mode === 'study' || playing) && !over;
  const moving: Side = settling ? kuroobi : pos.turn;
  const side = (s: Side): ScoreSide => ({
    count: s === 1 ? c.b : c.w,
    turn: going && moving === s,
    busy: mode === 'play' && s === kuroobi && thinking ? L('thinking') : undefined,
  });
  const R = (key: string) => t(lang, 'result.' + key);
  const winner: Side | 0 = c.b > c.w ? 1 : c.w > c.b ? 2 : 0;
  const result = !over || settling || (mode === 'play' && !playing) ? null
    : mode === 'study'
      ? { title: R(winner === 1 ? 'blackWin' : winner === 2 ? 'whiteWin' : 'draw'), tone: 'neutral' as const }
      : { title: R(winner === game.you ? 'youWin' : winner ? 'kuroobiWin' : 'draw'), tone: winner === game.you ? 'win' as const : winner ? 'loss' as const : 'neutral' as const };

  const shareUrl = () => location.origin + studyUrl(start, moves, at);
  const shareItems = [
    { label: L('shareLink'), icon: 'link' as const, onSelect: () => copy(shareUrl(), L('linkCopied')) },
    ...(moves.length || !isStart(start) ? [{ label: L('shareKifu'), icon: 'copy' as const, onSelect: () => copy(recordText(), L('kifuCopied')) }] : []),
    {
      label: L('shareX'), icon: 'x' as const, external: t(lang, 'site.newWindow'),
      onSelect: () => window.open(`https://x.com/intent/post?text=${encodeURIComponent(L('shareText'))}&url=${encodeURIComponent(shareUrl())}`, '_blank', 'noopener'),
    },
    // Only where the system has a share sheet (phones, Safari); elsewhere the item would do nothing.
    ...(typeof navigator !== 'undefined' && 'share' in navigator
      ? [{ label: L('shareMore'), icon: 'more' as const, onSelect: () => { navigator.share({ title: L('shareText'), url: shareUrl() }).catch(() => {}); } }]
      : []),
  ];

  return (
    <div className="pa">
      <BoardDefs />
      <div className="pa-bar">
        <h1 className="pa-title">Kuroobi for web</h1>
        <Segmented label={L('modes')} items={[
          { label: L('title'), href: href(lang, '/play/'), on: mode === 'play' },
          { label: L('studyTitle'), href: studyUrl(start, moves), on: mode === 'study' },
        ]} />
        <div className="pa-tools">
          <Button icon="load" aria-label={L('load')} aria-haspopup="dialog" onClick={() => setLoadOpen(true)}>{L('load')}</Button>
          <MenuButton label={L('share')} icon="share" items={shareItems} />
        </div>
      </div>

      {note && <Toast>{note}</Toast>}

      {loadOpen && <LoadRecord lang={lang} onLoad={applyLoad} onClose={() => setLoadOpen(false)} />}

      {failed && (
        <div role="alert" className="pa-failed">
          <span className="pa-strong">{L('loadFailed')}</span>
          <Button onClick={() => location.reload()} style={{ marginLeft: 'auto' }}>{L('reload')}</Button>
        </div>
      )}

      {!ready && !failed && (
        <div role="status" className="pa-loading">
          <span className="pa-strong">{L('loading')}</span>
          <span className="pa-loading-bar"><Progress value={progress?.total ? progress.loaded / progress.total : 0} label={L('loadingAria')} /></span>
          <span className="pa-mono">
            {progress ? `${(progress.loaded / 1e6).toFixed(1)}${progress.total ? ` / ${(progress.total / 1e6).toFixed(1)}` : ''} MB` : ''}
          </span>
        </div>
      )}

      <div className="pa-cols">
        <div className="pa-left">
          <div className={'pa-o-score' + (ready ? '' : ' pa-dim')}>
            <Scoreboard lang={lang} black={side(1)} white={side(2)} result={result ?? undefined} />
          </div>
          <div className="pa-o-board pa-board">
            <div className={ready ? undefined : 'pa-dim'} style={{ width: '100%', height: '100%' }}>
              <Board
                lang={lang}
                cells={pos.cells}
                legal={over || (mode === 'play' && (!playing || choosing)) ? [] : legal(pos.cells, pos.turn)}
                evals={boardEvals}
                turn={pos.turn === 1 ? 'b' : 'w'}
                last={at > 0 ? moves[at - 1] : null}
                coords
                disabled={!ready || (mode === 'play' && !yours)}
                onPlay={onPlay}
              />
            </div>
            {mode === 'play' && choosing && (
              <BoardCover title={L('setupTitle')}>
                <RadioCards legend={L('side')} name="side" value={setup.you} onChange={(you) => setSetup({ ...setup, you })}
                  options={[{ value: 1 as Side, label: L('sideBlack'), stone: 'b', hint: L('sideFirst') }, { value: 2 as Side, label: L('sideWhite'), stone: 'w', hint: L('sideSecond') }]} />
                <Strength lang={lang} label={L('strength')} value={setup.level} onChange={(level) => setSetup({ ...setup, level })} />
                <div className="pa-cover-actions">
                  <Button onClick={() => setChoosing(false)}>{L('cancel')}</Button>
                  <Button kind="primary" disabled={!ready} onClick={begin}>{L('begin')}</Button>
                </div>
              </BoardCover>
            )}
          </div>
          {shown && <div className="pa-o-legend"><EvalLegend lang={lang} /></div>}

          {gameButtons && (
            <div className="pa-o-act">
              {undoButton}
              {newGameButton}
              <div className="pa-o-view">
                <MenuButton label={L('view')} icon="view" items={viewItems} />
              </div>
            </div>
          )}

          {mode === 'study' && (
            <div className="pa-o-step">
              <StepNav lang={lang} at={at} total={moves.length} onJump={jump} />
            </div>
          )}

          {mode === 'study' && (
            <div className="pa-o-graph">
              <EvalGraph
                lang={lang}
                points={graph}
                plies={PLIES}
                cursor={at}
                blunder={whole ? undefined : blunder}
                busy={!!whole}
                onJump={(n) => { if (n <= moves.length) jump(n); }}
                moveName={(n) => (n >= 1 && n <= moves.length ? squareName(moves[n - 1]) : undefined)}
              />
            </div>
          )}
        </div>

        <div className="pa-right">
          {mode === 'play' && (
            <div className="pa-o-main pa-o-game">
              <Panel id="pa-game" title={L('game')}>
                {gameButtons && <div className="pa-buttons">{undoButton}{newGameButton}</div>}
                <Toggle label={L('showEvals')} checked={hintsOn} onChange={showEvals} />
                {playing && <>
                  <RadioCards legend={L('side')} name="side-now" value={game.you} disabled onChange={() => {}}
                    options={[{ value: 1 as Side, label: L('sideBlack'), stone: 'b', hint: L('sideFirst') }, { value: 2 as Side, label: L('sideWhite'), stone: 'w', hint: L('sideSecond') }]} />
                  <Strength lang={lang} label={L('strength')} value={game.level} disabled onChange={() => {}} />
                </>}
              </Panel>
            </div>
          )}

          {mode === 'study' && (
            <div className="pa-o-main">
              <Panel id="pa-cands" title={L('candidates')} note={readNote}>
                <CandidateList
                  moves={over || !legal(pos.cells, pos.turn).length ? [] : (shown?.moves ?? []).map((m) => ({ square: squareName(m.move), score: m.score }))}
                  empty={over || !legal(pos.cells, pos.turn).length ? L('noMoves') : !ready ? L('waiting') : whole ? L('afterAnalysis') : L('analyzing')}
                />
                <Strength lang={lang} label={L('analysisDepth')} value={studyLevel} onChange={setStudyLevel} disabled={!!whole} />
                {whole ? (
                  <div role="status" className="pa-analysis">
                    <div className="pa-analysis-row">
                      <Busy>{L('analyzingShort')}</Busy>
                      <span className="pa-mono">{whole.done} / {whole.total}</span>
                      <Button onClick={stopAll} style={{ marginLeft: 'auto' }}>{L('stop')}</Button>
                    </div>
                    <Progress value={whole.done / whole.total} label={L('analyzeAll')} />
                  </div>
                ) : (
                  <Button kind="primary" disabled={!ready || !moves.length} onClick={analyzeAll}>{L('analyzeAll')}</Button>
                )}
              </Panel>
            </div>
          )}

          <div className="pa-o-kifu">
          <Panel id="pa-kifu" title={L('kifu')}>
            <div className="pa-kifu">
              <KifuTable lang={lang} moves={rows} current={at} onSelect={mode === 'study' ? jump : undefined}
                columns={mode === 'study' ? 'eval' : 'reading'} />
            </div>
          </Panel>
          </div>
        </div>
      </div>
    </div>
  );
}
