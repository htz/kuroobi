import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { PlayDock } from './PlayDock';
import { useGame } from './state';
import { useGgs } from './ggs';
import { flipped, usePrefs } from './prefs';
import type { GameView } from './types';
import { api, ggsApi, jsLog, onApp, type ActivityView } from './api';
import { useActivity, useEngineSettings, useEngineTurn, useGraph, useHints, useLearnLog, useStartGame, type AskArgs } from './engine';
import { fmtSecs } from './ggs';
import { cellsOf, connOf, evalsOf, ggsPlaying, movesOf, navBadges, sqName } from './adapt';
import { AppFrame, Body, BottomPanel, Busy, Divider, Dock, Main, Overlay, StatusBar, StatusStat, Toolbar, WindowBar } from './components/layout';
import { GgsChat, GgsConsole, GgsScreen } from './GgsScreens';
import { Confirm, PasteKifu, Settings } from './Dialogs';
import { Board } from './components/board';
import { EvalGraph, MoveScrub, ScoreRow, StoneDot, srcIsBook, srcLabel } from './components/data';
import { GgsStatus, JobList, Meter, Nav, navLocal, StatusChip, ggsNav, Toasts, type NavId, type Toast } from './components/ggs';
import { Button, Progress, Segmented, Toggle } from './components/primitives';
import { Icon } from './components/Icons';
import { BookDock, BookPane, BookTree, useBookBrowse } from './BookScreen';
import { LearnLog } from './LearnLog';
import { KifuViewer } from './KifuViewer';
import { LEVELS } from './state';
import { t, useLang, tErr } from './i18n';

/* Play and study screens.
 *
 * Engine interaction lives in engine.ts (same as the old App); this
 * file only shapes state into the screen. */

/* Role choices carry a stone (rule 59): name + stone reads as
 * "KUROOBI takes white" with no extra words. Only "none" (human plays
 * both) has no stone — with one, it becomes indistinguishable from
 * "both". */
const sides = () => [
  { value: 'black' as const, label: <><StoneDot color="b" />{t('app.color.black')}</> },
  { value: 'white' as const, label: <><StoneDot color="w" />{t('app.color.white')}</> },
  {
    // Never overlap the two stones: at 9px they merge into one black
    // blob, and in light theme the white stone's rim vanishes.
    value: 'both' as const,
    label: <><span style={{ display: 'flex', gap: 2 }}>
      <StoneDot color="b" /><StoneDot color="w" />
    </span>{t('app.side.both')}</>,
  },
  { value: 'off' as const, label: t('app.side.off') },
];


/** Map to a currently reachable destination; never squat on a nav row
 *  that no longer exists. */
function reachable(raw: NavId, conn: ReturnType<typeof connOf>): NavId {
  if (conn === 'online') return raw === 'ggs-login' ? 'ggs-play' : raw;
  return raw.startsWith('ggs-') && raw !== 'ggs-login' ? 'ggs-login' : raw;
}

export function App() {
  // The whole tree re-renders on a language change; t() is a plain
  // function, so nothing below subscribes on its own.
  useLang();
  const { prefs, set: setPref } = usePrefs();
  /* Clock settings live in prefs — a preference, not game state: it
     should survive restarts, and changing mid-game would jump the
     clocks. */
  const g = useGame(prefs.clockSecs);
  const ggs = useGgs();
  const [navRaw, setNavRaw] = useState<NavId>('play');
  const conn = connOf(ggs.snap?.conn);
  /* Connection changes swap the whole GGS nav block (rule 10: one
   * login row disconnected, seven connected). The location outlives
   * its row, so a successful login once left the login screen up
   * while both bands said "connected". When the current row vanishes,
   * derive a reachable destination — derive, not setState, to keep
   * the connection-then-render order (an effect write inserts one
   * stale frame). */
  const nav = reachable(navRaw, conn);
  const study = nav === 'study';
  const isBook = nav === 'book';
  const isGgs = nav.startsWith('ggs');
  const [tab, setTab] = useState('record');
  /* The book destination holds two sheets — tree+board ("book") and
     the write-back ledger ("learn log") — behind one nav row. With
     separate windows abandoned, §7 and §8 live here. */
  const [bookTab, setBookTab] = useState('book');
  const [dockOpen, setDockOpen] = useState(false);
  /* Eval-graph band; collapses only <=620px (base.css). Wide windows
     ignore this value, so a graph closed while short reappears when
     the window grows. */
  const [graphOpen, setGraphOpen] = useState(false);
  /* Viewpoint (design §2's black/white view): the eval SIGN, not
     board rotation, applied consistently to table, graph and board.
     Only study can switch it — flipping signs mid-game changes what
     the numbers mean while you play. */
  const [pov, setPov] = useState<'b' | 'w'>('b');

  // Settings used to be another window; listen for the change notice
  // and re-fetch only book presence (the board's book display).
  const setHasBook = g.setHasBook;
  useEffect(() => {
    const off = onApp('resources-changed', () => {
      void api.hasBook().then(setHasBook).catch(() => { /* engine not started yet */ });
    });
    return () => { void off.then((f) => f()); };
  }, [setHasBook]);

  useEngineSettings(g);
  useHints(g);
  useEngineTurn(g);
  const cpu = useActivity();
  // engine.ts has no UI; we show the confirmation and return the
  // answer.
  const [ask, setAsk] = useState<(AskArgs & { done: (ok: boolean) => void }) | null>(null);
  const confirm = useCallback(
    (a: AskArgs) => new Promise<boolean>((done) => setAsk({ ...a, done })),
    [setAsk]);
  // GGS games take priority; local games and analysis are refused
  // while one runs.
  const ggsMatch = ggsPlaying(ggs.snap);
  const graph = useGraph(g, ggsMatch, confirm);
  const startGame = useStartGame(g, ggsMatch, graph, confirm);

  // Chat unread count: 0 while open; the seen marker advances on
  // leave.
  /* During a GGS game, chat/console open from the status bar's right
   * edge. They exist as nav destinations too, but you don't want to
   * leave the board mid-game — one panel, board in view (rule 12). */
  const [panel, setPanel] = useState<'' | 'chat' | 'console'>('');

  /* The seen marker is a timestamp, persisted to disk. An in-memory
   * count reset every launch, so the 300 replayed messages arrived as
   * unread each time; counts also drift when history is trimmed. */
  const chatMsgs = ggs.snap?.chat ?? [];
  const chatSeen = ggs.snap?.chat_seen ?? 0;
  const chatLatest = chatMsgs.length ? Math.max(...chatMsgs.map((m) => m.at)) : 0;
  // The bottom-panel chat counts as reading too, same as the nav
  // destination — else unread grows while the panel sits open.
  const chatOpen = nav === 'ggs-chat' || panel === 'chat';
  const chatUnread = chatOpen ? 0 : chatMsgs.filter((m) => m.at > chatSeen).length;

  /** Advance the seen marker (never backwards; ggs.rs enforces too). */
  const markChatRead = useCallback(() => {
    if (chatLatest > chatSeen) void ggsApi.chatSeen(chatLatest);
  }, [chatLatest, chatSeen]);

  /* Messages arriving while open count as read; advancing only on
   * leave turns an idle-open backlog into instant unread. */
  useEffect(() => {
    if (chatOpen) markChatRead();
  }, [chatOpen, markChatRead]);

  /** Switch the bottom panel; leaving chat advances the seen marker. */
  const showPanel = useCallback((next: '' | 'chat' | 'console') => {
    setPanel((cur) => {
      if (cur === 'chat' && next !== 'chat') markChatRead();
      return cur === next ? '' : next;
    });
  }, [markChatRead]);

  // Destination and engine mode are one thing; desynced, the engine
  // moves during study.
  const setMode = g.setMode;
  const setNav = useCallback((id: NavId) => {
    // Advance on leaving chat; only what arrives after counts as
    // unread.
    if (navRaw === 'ggs-chat' && id !== 'ggs-chat') markChatRead();
    setNavRaw(id);
    if (id === 'play' || id === 'study') setMode(id === 'study' ? 'study' : 'vs');
  }, [setMode, navRaw, markChatRead]);

  // The dock's learning tab shows the position count too.
  const book = useBookBrowse(isBook || tab === 'learn');
  const { items: learnLog, reload: learnLogReload } = useLearnLog(
    isBook && bookTab === 'log', !!cpu?.learn);

  /* Record viewer (rule 71). pending is an archive id — games with
   * no local record open the overlay first and fill in on arrival. */
  const [viewer, setViewer] = useState<
    { title: string; kifu: string; pending?: string; archive?: string; parts?: string[] } | null
  >(null);

  const [paste, setPaste] = useState(false);
  /* Settings are an overlay; the separate window was abandoned. Only
     the Display tab benefited, it still overlapped the board 12% of
     the time at 1240px, and it dragged in localStorage sync, window
     badges and placement math. Not worth it (needs a design push —
     §5 still says "separate window (⌘,)"). */
  const [settings, setSettings] = useState(false);
  /** Initial settings tab (screenshot-only entry). */
  const [settingsTab, setSettingsTab] = useState<'engine' | 'view' | 'ggs'>('engine');

  /* Verification hook (KUROOBI_AUTOPLAY=both:11). This repo verifies
   * screens by launch-and-capture, so the entry stays. "study" loads
   * a record and opens study; "settings" opens settings. */
  const started = useRef(false);
  // For "study:graph": graph.update is created after this effect, so
  // it passes through a ref (as a dep it would re-run every time).
  const autoGraph = useRef<(() => void) | null>(null);
  const bookLine = useRef<((kifu: string) => void) | null>(null);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void api.autoplay().then(async (v) => {
      if (!v) return;
      const [who, lv, extraRaw] = v.split(':');
      /* :nobook is a trailing flag, not a dock tab. Positional parsing
         made both:40:nobook select a tab named "nobook" — matching
         nothing, blank screen (this happened). Treat it as a flag
         wherever it appears. */
      // Trailing flags (nobook / clock<seconds>) are not tab names.
      const extra =
        extraRaw === 'nobook' || /^clock\d+$/.test(extraRaw ?? '') ? undefined : extraRaw;
      /* :nobook disables the book before starting, and must apply
         before ANY entry — the study branch returns, so placed lower
         it reached some entries and not others (study:graph:nobook
         was not cutting it). In games, book-served openings skip the
         search, so Activity.local never rises and the "yielding" row
         can't be captured; in analysis it toggles the book dots. */
      if (v.endsWith(':nobook')) g.setUseBook(false);
      /* both:6:clock60 — trailing flag like nobook: start with a
         clock (capture-only entry; before it, clocks could not be
         verified without clicking through settings). Also calls
         api.setClock directly — this path skips "new game", so
         changing prefs alone never initializes the clocks. */
      const mc = v.match(/:clock(\d+)$/);
      if (mc) { setPref('clockSecs', +mc[1]); void api.setClock(+mc[1]); }
      if (who === 'settings') {
        // A tab can be given, e.g. settings:ggs (capture only).
        if (lv === 'ggs' || lv === 'view' || lv === 'engine') setSettingsTab(lv);
        setSettings(true);
        return;
      }
      /* Overlay entries (capture only): overlays are click-only and
         their dimensions were never measurable — confirm / record
         loader / record viewer. Use overlay:confirm etc. */
      if (who === 'overlay') {
        if (lv === 'paste') { setPaste(true); return; }
        if (lv === 'confirm') {
          void confirm({ title: t('app.autoplay.undo_title'),
                         body: t('app.autoplay.undo_body'),
                         ok: t('app.autoplay.undo_ok') });
          return;
        }
        if (lv === 'toast') {
          // Capture two stacked (design §9 overlaps two with a 10px
          // gap).
          g.say(t('app.autoplay.toast_ggs_busy'), 'gold');
          setTimeout(() => g.say(t('app.study.no_record')), 150);
          return;
        }
        if (lv === 'viewer') {
          setViewer({ title: t('app.autoplay.viewer_title'),
                      kifu: 'e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2' });
          return;
        }
        return;
      }
      /* "yield" — capture entry for learning-yielding. learn_paused
         is true only while Activity.local is up, but book openings
         skip the search and a game's import (60 positions) finishes
         before midgame. The overlap must be manufactured in one
         process: game ends -> import starts -> search starts again.
         Wait for the import, then overlay study analysis. */
      if (who === 'yield') {
        setTab('learn');
        g.setSide('both');
        g.setLevel(0);   // Fastest: end the game early so the import starts
        g.setPlaying(true);
        void (async () => {
          for (let i = 0; i < 240; i++) {
            await new Promise((r) => setTimeout(r, 500));
            const a = await api.activity().catch(() => null);
            if (a?.learn) break;
          }
          /* One more game, read deeper this time. Study analysis won't
             overlap — per-position flags are brief and cached values
             finish instantly. Only seconds-per-move search sustains
             the yield. */
          await g.newGame();
          // Disable the book: book-served openings skip the search, so
          // no depth yields from move one.
          g.setUseBook(false);
          g.setLevel(12);
          g.setPlaying(true);
        })();
        return;
      }
      // "tab:<name>" selects a dock tab (capture only);
      // "tab:<strength-tab>:custom" opens the three custom fields.
      if (who === 'tab') {
        if (lv) setTab(lv);
        if (v.endsWith(':custom')) { g.setCustom({ depth: 20, solve: 24, band: 4 }); g.setLevel('custom'); }
        return;
      }
      // "book:f5d6" opens the book walked to that node; "book:log"
      // opens the second sheet (learn log). Capture only.
      if (who === 'book') {
        setNavRaw('book');
        if (lv === 'log') setBookTab('log');
        else if (lv) bookLine.current?.(lv);
        return;
      }
      if (who === 'study') {
        // Wait a beat or the startup state fetch overwrites this with
        // the initial position.
        await new Promise((r) => setTimeout(r, 500));
        setNavRaw('study');
        g.setMode('study');
        g.setView(await api.loadKifuText(
          'e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2'));
        // "study:hint" also enables eval display on an in-book
        // opening position.
        if (lv === 'hint') {
          g.setView(await api.goto(8));
          g.setAutoHint(true);
        }
        // "study:graph" starts analysis too (to check the progress UI).
        if (lv === 'graph') setTimeout(() => autoGraph.current?.(), 400);
        // A dock tab may follow. Analysis raises Activity.local, so
        // the learning-yield look is capturable here.
        if (extra) setTab(extra);
        return;
      }
      if (who === 'both') g.setSide('both');
      if (lv !== undefined && Number.isFinite(+lv)) g.setLevel(+lv);
      // A dock tab may follow; the import status shows only while
      // learning runs post-game, and this captures that window.
      if (extra) setTab(extra);
      g.setPlaying(true);
    }).catch((e) => jsLog('autoplay: ' + e));
    // g is rebuilt every render; not a dep (started limits to once).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => { autoGraph.current = () => void graph.update(); }, [graph]);
  useEffect(() => { bookLine.current = book.goto; }, [book.goto]);

  /* Screen verification (KUROOBI_GGS_AUTOVIEW=players): GGS screens
   * render only when opened, so capture needs a destination hook. */
  /* Env-var overrides, read once at launch (they never change). */
  const [envOverrides, setEnvOverrides] = useState<[string, string][]>([]);
  useEffect(() => { void api.envOverrides().then(setEnvOverrides).catch(() => {}); }, []);

  useEffect(() => {
    void ggsApi.autoview().then((v) => {
      // In-screen state may follow ("users:card"); the destination is
      // the first segment.
      const to = v.split(':')[0];
      if (to) setNavRaw(('ggs-' + to) as NavId);
    }).catch(() => {});
  }, []);

  /** Loading clears the move records too (stale evals would lie). */
  const applyLoaded = (v: GameView) => {
    g.setMoveSource({});
    g.setThinkTotal({ black: 0, white: 0 });
    g.setPlaying(false);
    g.setView(v);
  };
  const loadFromFile = async () => {
    try {
      const loaded = await api.loadKifu();
      if (loaded) { applyLoaded(loaded); setPaste(false); }   // null = closed without choosing
    } catch (e) { g.say(tErr(e)); }
  };
  /** Load and optionally advance to a ply (jumps from the ledger). */
  const loadFromText = async (text: string, ply?: number) => {
    try {
      applyLoaded(await api.loadKifuText(text));
      setPaste(false);
      if (ply !== undefined) await g.jumpTo(ply);
    } catch (e) { g.say(tErr(e)); }
  };

  /* Receive GGS notices and fetched records. The backend leaves both
   * in place, so consume-and-clear — else reopening replays the same
   * notice and the next fetch is indistinguishable. */
  const notice = ggs.snap?.notice ?? '';
  const fetched = ggs.snap?.fetched_ggf ?? null;
  useEffect(() => {
    if (!notice) return;
    void (async () => {
      g.say(tErr(notice));
      await ggsApi.ack().catch(() => {});
    })();
    // g is rebuilt every render; not a dep (runs on notice change).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [notice]);
  useEffect(() => {
    if (!fetched) return;
    void (async () => {
      // If the overlay is waiting, feed it; only otherwise go to
      // study.
      if (fetched.ggf) {
        // Synchro archives hold two boards; pass all so the overlay
        // can choose.
        setViewer((cur) => (cur && cur.pending === fetched.id
          ? { ...cur, kifu: fetched.ggf, parts: fetched.parts }
          : cur));
        // Study only when no overlay waits. viewer is not a dep (it
        // would re-run on every open).
        if (!viewer?.pending) {
          setNav('study');
          await loadFromText(fetched.ggf);
        }
      } else {
        setViewer(null);
        g.say(fetched.error ? tErr(fetched.error) : t('app.ggs.fetch_failed'));
      }
      await ggsApi.ack().catch(() => {});
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fetched]);

  const v = g.view;
  // Negate only in study; play, book and GGS stay black-view.
  const sign: 1 | -1 = study && pov === 'w' ? -1 : 1;
  const moves = useMemo(
    () => (v ? movesOf(v, g.moveSource, graph.values, sign) : []),
    [v, g.moveSource, graph.values, sign]);
  const evals = g.autoHint ? evalsOf(g.hints, v?.player !== 'white', sign) : undefined;
  /* Graph points negate with the viewpoint too; swapping labels while
     keeping values lies — white view showed black-better points
     rising toward "white better" (found live). */
  const povPoints = useMemo(
    () => (graph.values ?? []).map((p) => (p && sign === -1 ? { ...p, value: -p.value } : p)),
    [graph.values, sign]);
  // Losing move = costliest move, computed once so strip and graph
  // agree.
  const blunder = useMemo(() => {
    let best: { at: number; loss: number } | undefined;
    for (const m of moves) if (m.loss && (!best || m.loss > best.loss)) best = { at: m.n, loss: m.loss };
    return best;
  }, [moves]);

  /* Study shows the current move's eval in the disc row (design §2).
     The graph shows shape, not values you can read off dots; one
     number per step reads while you walk. moves already carries
     move/color/eval/loss. */
  const cur = v && v.cursor > 0 ? moves[v.cursor - 1] : undefined;
  const curMoveMeta = cur && cur.score !== undefined ? (
    <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
      {t('app.study.move_eval')}
      <b style={{
        fontSize: 'var(--fs-3)', fontWeight: 600, color: 'var(--text)',
        fontVariantNumeric: 'tabular-nums',
      }}>{cur.score > 0 ? '+' : ''}{cur.score.toFixed(1)}</b>
      {/* Loss speaks in color; bare numbers read as "good move, low
          value". */}
      {!!cur.loss && (
        <span style={{ color: 'var(--bad)', fontVariantNumeric: 'tabular-nums' }}>
          ▼{cur.loss.toFixed(1)}
        </span>
      )}
      <span>{cur.pass ? t('app.moves.pass') : cur.move} · {t(cur.color === 'b' ? 'app.color.black' : 'app.color.white')}</span>
      {cur.src && <span style={{ color: srcIsBook(cur.src) ? 'var(--gold)' : 'var(--sub)' }}>{srcLabel(cur.src)}</span>}
    </span>
  ) : undefined;

  /* Keyboard. Inert while typing (chat, console, record paste) and
   * while any overlay is up — typed letters would switch screens.
   * Overlays own their keys (Esc etc.). */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return;
      /* Any overlay disables keys. Checking only paste+confirm let
         arrows move the board behind the viewer and settings; count
         overlays in one place instead of appending per overlay. */
      if (paste || ask || viewer || settings) return;
      const cmd = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();
      /* ⌘, opens settings — §5 spells it out, it is macOS convention,
         and people try it before looking. */
      if (cmd && key === ',') { e.preventDefault(); setSettings(true); return; }
      if (cmd && key === 'b') { e.preventDefault(); setNav('book'); return; }
      if (cmd && key === 'n') { e.preventDefault(); if (!g.thinking) void g.newGame(); return; }
      // Record in/out; GGS and book have no record. Refuse under the
      // SAME condition as the button — a shortcut that works while the
      // button is disabled contradicts the disabled state.
      if (cmd && key === 's' && !isGgs && !isBook) {
        e.preventDefault();
        if (v && v.moves.length > 0) {
          void api.saveKifu(...ggfNames(g.side)).catch((err) => g.say(tErr(err)));
        }
        return;
      }
      if (cmd && key === 'o' && !isGgs && !isBook) { e.preventDefault(); setPaste(true); return; }
      if (cmd && key === 'z') {
        e.preventDefault();
        if (!g.thinking && v && v.move_count > 0) void g.undo();
        return;
      }
      // Only study and book navigate plies; mid-game arrows blur
      // "undone" vs "vanished".
      if (isBook) {
        if (e.key === 'ArrowLeft') { e.preventDefault(); book.back(); }
        if (e.key === 'ArrowUp') { e.preventDefault(); book.reset(); }
        // Right = advance along the best move; traces the main line.
        if (e.key === 'ArrowRight' && book.node?.moves.length) {
          e.preventDefault();
          book.push(book.node.moves[0].pos);
        }
        return;
      }
      if (!study || !v) return;
      // ⌘ to the end, ⇧ by 10, plain by 1.
      const step = e.shiftKey ? 10 : 1;
      if (e.key === 'ArrowLeft') { e.preventDefault(); void g.jumpTo(cmd ? 0 : Math.max(0, v.cursor - step)); }
      if (e.key === 'ArrowRight') {
        e.preventDefault();
        void g.jumpTo(cmd ? v.moves.length : Math.min(v.moves.length, v.cursor + step));
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [setNav, paste, ask, viewer, settings, isBook, isGgs, study, book, g, v]);

  const toasts: Toast[] = g.toasts.map(x => ({ id: String(x.id), tone: x.tone, text: x.text }));

  const over = v?.over ?? false;
  const result = v?.over
    ? t(v.black === v.white ? 'app.result.draw'
        : v.black > v.white ? 'app.result.black_wins' : 'app.result.white_wins')
    : undefined;
  const anyThink = g.thinkTotal.black > 0 || g.thinkTotal.white > 0;
  /** Clock for the disc row: remaining if timed, else cumulative
   *  think time. */
  const clockLabel = (c: 'b' | 'w') => {
    if (g.clockSecs) {
      const v = c === 'b' ? g.clock?.black : g.clock?.white;
      return v === undefined ? undefined : fmtSecs(v);
    }
    return !study && anyThink ? fmtSecs(c === 'b' ? g.thinkTotal.black : g.thinkTotal.white) : undefined;
  };


  const nodes = g.stat && g.stat.nodes > 0 ? g.stat.nodes : 0;
  const nps = nodes && g.stat && g.stat.secs > 0 ? nodes / g.stat.secs : 0;
  const lv = g.level === 'custom' ? t('app.level.custom') : LEVELS[g.level].name;
  /* WindowBar's "what am I looking at": passive text (rule 75), and
   * the same names as the nav — two copies would drift. */
  const screenTitle =
    [...navLocal(), ...ggsNav(conn)].find((i) => i.id === nav)?.label ?? 'KUROOBI';
  const screenSub = isGgs
    ? (conn === 'online' ? ggs.snap?.login : undefined)
    : isBook ? undefined
    : `${lv} · ${t(g.side === 'both' ? 'app.side.both'
        : g.side === 'off' ? 'app.side.none_assigned'
        : g.side === 'black' ? 'app.color.black' : 'app.color.white')}`;

  // "Me at the bottom" = the color KUROOBI does NOT hold.
  const sideColor = g.side === 'black' ? 'white' : g.side === 'white' ? 'black' : '';

  return (
    <AppFrame>
      {/* Window layer; nothing clickable (rule 75). */}
      {/* Env-var badges live at the WindowBar's right edge. Env-vars
          change behavior (rated play blocked, fake server, different
          archive dir), and without the badge you cannot tell a real
          screen from a test one. The old nav-bottom spot was doubly
          wrong: that is the resource meters' place (rule 9) and
          --gold means book (rule 19). The WindowBar is the same spot
          on every screen and immune to the collapse tiers. */}
      <WindowBar title={screenTitle} sub={screenSub} right={<EnvTags items={envOverrides} />} />

      <Body>

      <Nav items={navLocal()} ggsItems={ggsNav(conn, navBadges(ggs.snap, chatUnread))} conn={conn}
           active={nav} onSelect={setNav}
           footer={<>
             {cpu && <>
             {/* Usage caps at cores x 100%; the bar fills by that
                 fraction. */}
             <Meter icon="cpu" label="CPU" value={Math.round(cpu.cpu)} unit="%"
                    ratio={cpu.cpu / (cpu.cores * 100)} />
             {/* Flex collapses the space before the unit; use a
                 non-breaking one. */}
             <Meter icon="memory" label={t('app.meter.memory')} value={(cpu.mem / 1e9).toFixed(1)} unit={'\u00a0GB'}
                    ratio={cpu.mem_total > 0 ? cpu.mem / cpu.mem_total : 0} />
             <JobList jobs={jobsOf(cpu)} />
             </>}
             {/* Settings sit at the very bottom, outside the nav rows
                 (not a destination). Not a gear icon — GGS settings
                 use the gear, and one glyph gets one meaning (rule
                 49). The 48px rail drops the label. */}
             <button type="button" className="k-press k-nav-settings"
                     title={t('app.nav.settings_title')} aria-label={t('app.nav.settings')}
                     onClick={() => setSettings(true)}
                     style={{
                       alignItems: 'center', justifyContent: 'center',
                       gap: 'var(--sp-2)', height: 'var(--h-field)',
                       border: '1px solid var(--border)', borderRadius: 'var(--r-2)',
                       background: 'var(--card)', color: 'var(--text)',
                       fontSize: 'var(--fs-6)', cursor: 'pointer', padding: 0,
                     }}>
               <Icon name="prefs" size={15} />
               <span className="k-nav-label">{t('app.nav.settings')}</span>
             </button>
           </>} />

      <Main inset={dockOpen && !isGgs}>
      <Toolbar
          dock={isGgs ? undefined : { open: dockOpen, onToggle: () => setDockOpen(o => !o) }}
          graph={study && !isGgs && !isBook
            ? { open: graphOpen, onToggle: () => setGraphOpen(o => !o) } : undefined}
          /* The book screen shows import progress on the right (§8) —
             this screen exists to kill "it's running but what
             changed", so the current position count is essential.
             The row disappears when idle (rule 11). */
          aux={isBook
            ? (cpu?.learn
              ? <Busy>{t('app.book.importing')}<span style={{ color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                    {t('app.book.import_count', { done: cpu.learn[0].toLocaleString(),
                                                  total: cpu.learn[1].toLocaleString() })}
                  </span>
                </Busy>
              : undefined)
            : isGgs ? (conn === 'online' && ggs.snap
              ? <GgsStatus snap={ggs.snap}
                           showStrength={nav !== 'ggs-settings' && nav !== 'ggs-standby'} />
              : undefined)
            : undefined}>
          {isBook ? (
            <>
              {/* The sheet switch goes in children — aux dies at 940px
                  and would strand the learn log (rules 8/58). */}
              <Segmented value={bookTab} onChange={setBookTab}
                         options={[{ value: 'book', label: t('app.book.tab_book') },
                                   { value: 'log', label: t('app.book.tab_learn_log') }]} />
              {bookTab === 'book' && <>
                <Divider />
                <Button disabled={!book.line.length} onClick={book.back}>{t('app.book.back')}</Button>
                <Button disabled={!book.line.length} onClick={book.reset}>{t('app.book.reset')}</Button>
                <span style={{ marginLeft: 'var(--sp-3)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                  {book.line.length ? t('app.book.ply', { n: book.line.length }) : t('app.book.initial')}
                </span>
              </>}
            </>
          ) : isGgs ? (
            <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>
              {conn === 'online' ? <>{t('app.ggs.connected')} <b style={{ color: 'var(--text)' }}>{ggs.snap?.login}</b></>
                : t(conn === 'offline' ? 'app.ggs.offline' : 'app.ggs.logging_in')}
            </span>
          ) : study ? (
            // KUROOBI never moves in study. Step buttons moved to the
            // scrubber (design §2): navigation tools stay together, and
            // apart, ▶'s effect appears elsewhere. Analysis lives in
            // the graph heading (same reason). The design also puts
            // analyze / load record / viewpoint here — all elsewhere
            // now (ledgered). */
            <>
              <Button variant="primary" disabled={graph.busy || !v?.moves.length}
                      onClick={() => void graph.update()}>{t('app.study.analyze')}</Button>
              <Button title="⌘O" onClick={() => setPaste(true)}>{t('app.study.load_record')}</Button>
              <Divider />
              <Segmented value={pov} onChange={setPov}
                         options={[{ value: 'b', label: t('app.study.black_view') },
                                   { value: 'w', label: t('app.study.white_view') }]} />
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                {v && v.moves.length
                  ? t('app.study.ply_of', { n: v.cursor, total: v.moves.length })
                  : t('app.study.no_record')}
              </span>
            </>
          ) : (
            <>
              <Button variant={g.playing ? 'danger' : 'primary'}
                      disabled={!g.playing && (over || g.thinking)}
                      onClick={startGame}>
                {t(g.playing ? 'app.play.stop' : 'app.play.start')}
              </Button>
              {/* Shortcuts go on the button: clickability is visible,
                  the shortcut is not. */}
              <Button title="⌘N" disabled={g.thinking} onClick={() => void g.newGame()}>{t('app.play.new_game')}</Button>
              <Button title="⌘Z" disabled={g.thinking || !v || v.move_count === 0}
                      onClick={() => void g.undo()}>{t('app.play.undo')}</Button>
              {/* Rule between actions and game setup — without it "new
                  game" and "black" read as one row. */}
              <Divider />
              {/* Role must stay editable in narrow windows: children,
                  not aux (aux dies at 940px). */}
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>KUROOBI</span>
              <Segmented value={g.side} onChange={g.setSide} options={sides()} />
              {/* Eval display toggle likewise in children — in aux it
                  vanished entirely at 900px (verified live). */}
              <Divider />
              <Toggle checked={g.autoHint} onChange={g.setAutoHint} label={t('app.play.show_evals')} />
            </>
          )}
      </Toolbar>
        {/* The board's top band holds game actions and minimal setup;
            thinking numbers go to the status bar. */}

        {isGgs ? <GgsScreen nav={nav} snap={ggs.snap} onNav={setNav} prefs={prefs}
                       onKifu={(title, kifu, archive) => {
                         /* Fetch from the archive when an id exists:
                            synchro ids hold two boards while the local
                            record has one, and archive records carry
                            evals and clocks. Local is the id-less
                            fallback. */
                         if (archive) {
                           setViewer({ title, kifu: '', pending: archive, archive });
                           void ggsApi.look(archive);
                         } else {
                           setViewer({ title, kifu });
                         }
                       }} />
         : isBook ? (
          bookTab === 'book' ? (
            /* Design §7 is three columns: tree / board / this
               position. The tree takes the left, "this position" and
               "next moves" take the right dock (290 ≈ the design's
               291). The nav makes the board narrower than drawn. */
            <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
              {book.node?.size !== 0 && (
                <BookTree b={book} decimals={prefs.decimals}
                          onStudy={(kifu) => { setNav('study'); void loadFromText(kifu); }} />
              )}
              <BookPane b={book} coords={prefs.coords} grain={prefs.grain}
                        flip={flipped(prefs.facing, '')} onSettings={() => setSettings(true)} />
            </div>
          ) : (
            /* The write-back ledger shares the book screen so game ->
               losing move -> old-to-new -> revert reads as one line. */
            /* Design §8 is three panes (game list / losing-move board
               / this game's detail); the container leaves widths to
               the parts (269 / center / 290). */
            <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
              <LearnLog items={learnLog}
                onBook={(kifu) => { setBookTab('book'); book.goto(kifu); }}
                onUndo={(e) => void (async () => {
                  if (!await confirm({
                    title: t('app.learn.undo_title'),
                    body: t('app.learn.undo_body'),
                    ok: t('app.learn.undo_ok'), danger: true,
                  })) return;
                  try {
                    await api.learnUndo(e.at, e.kifu);
                    learnLogReload();
                  } catch (err) { g.say(tErr(err)); }
                })()}
                onOpen={(e, ply) => {
                  setNav('study');
                  void loadFromText(e.start ? e.start + '\n' + e.kifu : e.kifu, ply);
                }} />
            </div>
          )
        ) : (
        // Side padding belongs to the children, or the disc row's rule
        // stops short of the edges (the design runs edge to edge).
        // The min-height lives on THIS tier: on the board alone, flex
        // stretches the outside and lets the content overflow onto the
        // scrubber and graph. Here, flex learns of the shortage and
        // shrinks the graph first.
        <div style={{
          flex: 1, minHeight: 'calc(200px + var(--h-bar))',
          display: 'flex', flexDirection: 'column',
        }}>
          {/* minmax(0,1fr) pins the row height. Default auto sizes the
              row by content, leaving inner height:100% without a base
              — the svg renders at its intrinsic 880px and overflows
              downward, noticed only when it covers the disc row
              (rule 77). */}
          {/* Board min-size (rule 7): with a rigid graph band, the
              minimum window crushed the board to 82px. The board
              collapses last; shrink the graph first. */}
          <div style={{
            flex: 1, minHeight: 200, display: 'grid', placeItems: 'center',
            gridTemplateRows: 'minmax(0, 1fr)', gridTemplateColumns: 'minmax(0, 1fr)',
            padding: 'var(--sp-2) var(--sp-4)',
          }}>
            {/* maxHeight is required too: in short containers the grid
                row sizes to content and the svg renders at 880px,
                overflowing downward (rule 77). */}
              <div style={{ height: '100%', maxHeight: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
              {v && <Board cells={cellsOf(v)} legal={v.legal} last={v.last} evals={evals}
                           coords={prefs.coords} grain={prefs.grain}
                           // Study has no "my color"; auto keeps black
                           // at the bottom.
                           flip={flipped(prefs.facing, study ? '' : sideColor)}
                           disabled={g.thinking}
                           onPlay={(sq) => void g.play(sq)} />}
            </div>
          </div>
          <ScoreRow black={v?.black ?? 2} white={v?.white ?? 2}
                    turn={!v || v.over ? undefined : v.player === 'black' ? 'b' : 'w'}
                    meta={study ? curMoveMeta : result}
                    /* Remaining time when clocked, else cumulative
                       think time — in timed games "seconds left" is
                       what you read. */
                    blackClock={clockLabel('b')} whiteClock={clockLabel('w')} />
        </div>
        )}

        {/* Bottom panel during GGS games; chat and console share it. */}
        {isGgs && panel && ggs.snap && (
          <BottomPanel
            tabs={[{ id: 'chat', label: t('app.panel.chat'), unread: panel === 'chat' ? 0 : chatUnread },
                   { id: 'console', label: t('app.panel.console') }]}
            active={panel} onTab={(id) => showPanel(id as 'chat' | 'console')}
            onClose={() => showPanel('')}>
            {panel === 'chat' ? <GgsChat snap={ggs.snap} /> : <GgsConsole snap={ggs.snap} />}
          </BottomPanel>
        )}

        {/* Move scrubber; works without analysis, so it precedes the
            graph. */}
        {study && !isGgs && !isBook && v && (
          <MoveScrub plies={v.moves.length} cursor={v.cursor} blunder={blunder}
                     onSeek={(n) => void g.jumpTo(n)} />
        )}

        {/* Unboxed full-width band under the board; study only. */}
        {study && !isGgs && !isBook && (
          <EvalGraph points={povPoints} plies={v?.moves.length} cursor={v?.cursor}
                     blunder={blunder} busy={graph.busy} onJump={(n) => void g.jumpTo(n)}
                     open={graphOpen} pov={pov}
                     // Dot n is the position after n moves; the move
                     // played is the nth.
                     moveName={(n) => { const m = v?.moves[n - 1]; return m == null ? undefined : sqName(m); }}
                     extra={<>
                       {/* Progress goes in the heading row without
                           changing the band height — growth would
                           rattle everything below. */}
                       {/* A thin bar next to the number (rule 69);
                           digits alone give no sense of the
                           remainder. */}
                       {graph.prog && (
                         <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
                           {t('app.status.analyzing')} <b style={{ color: 'var(--text)' }}>{graph.prog.done}</b>/{graph.prog.total}
                           <span style={{ width: 72 }}>
                             <Progress value={graph.prog.total > 0 ? graph.prog.done / graph.prog.total : 0} />
                           </span>
                         </span>
                       )}
                       {/* Stop lives here, start in the toolbar (§2).
                           Shown only while running — never draw a stop
                           for something idle. Start used to live in
                           this band, which vanishes below 620px. */}
                       {graph.busy && (
                         <Button variant="danger" onClick={() => graph.stop()}>{t('app.study.stop_analysis')}</Button>
                       )}
                     </>} />
        )}

      </Main>

      {/* GGS has no dock (its list docks left of the body). */}
      {/* No empty dock without a book (the board side explains). */}
      {isBook && bookTab === 'book' && book.node?.size !== 0 && (
        <Dock tabs={[t('app.book.tab_book')]} active={t('app.book.tab_book')} open={dockOpen}>
          <BookDock b={book} decimals={prefs.decimals} />
        </Dock>
      )}

      {/* The record tab never scrolls whole: the table pins its header
          and controls, scrolling rows only. */}
      {!isGgs && !isBook && (
        <PlayDock g={g} book={book} cpu={cpu} prefs={prefs} tab={tab} onTab={setTab}
                  open={dockOpen} onNav={setNav} onBookTab={setBookTab}
                  onPaste={() => setPaste(true)} onLoadFile={loadFromFile}
                  study={study} moves={moves} ggfNames={() => ggfNames(g.side)} />
      )}

      </Body>

      {/* Only short fixed-width items; unpredictable notices go to
          toasts. */}
        <StatusBar
          left={<>
            {/* Analysis never raises g.thinking (it calls api.evalAt
                per position), so no running indicator existed. The
                graph heading's progress is band-internal; machine
                activity belongs to the status bar (rules 11/76). */}
            {graph.busy && (
              <Busy>{t('app.status.analyzing')}</Busy>
            )}
            {/* The design separates the state dot from the seconds;
                folding them into one "3.2s" leaves activity legible
                only by the number's presence (rule 11). */}
            {g.thinking && (
              <Busy>{t('app.status.thinking')}</Busy>
            )}
            {g.thinking && <StatusStat value={g.thinkSecs.toFixed(1)} unit="s" />}
            {nodes > 0 && <StatusStat label="nodes" value={fmtNodes(nodes)} />}
            {nps > 0 && <StatusStat label="nps" value={(nps / 1e6).toFixed(1)} unit="Mnps" />}
          </>}
          right={isGgs
            ? <>
              {/* Game-time only; otherwise it just duplicates the nav. */}
              {ggsMatch && <>
                <StatusChip label={t('app.panel.chat')} unread={panel === 'chat' ? 0 : chatUnread}
                            active={panel === 'chat'}
                            onClick={() => showPanel('chat')} />
                <StatusChip label={t('app.panel.console')} active={panel === 'console'}
                            onClick={() => showPanel('console')} />
              </>}
              <StatusStat label="GGS" value={t(conn === 'online' ? 'app.ggs.connected'
                : conn === 'offline' ? 'app.ggs.offline' : 'app.ggs.connecting')} />
            </>
            : isBook
            ? <>
              {/* Design §7's status bar shows positions / of-which-
                  learned. The latter appeared nowhere on the book
                  screen before. The design repeats both in the
                  toolbar; we don't (same number twice per screen —
                  needs a push). */}
              <StatusStat label={t('app.status.stored_positions')}
                          value={book.node ? book.node.size.toLocaleString() : '—'} />
              <StatusStat label={t('app.status.of_which_learned')}
                          value={book.node ? book.node.learned_size.toLocaleString() : '—'} />
            </>
            : <>
              {/* Study needs the exact ply; the scrubber ticks every
                  10, so the precise number lives here (rule 58). */}
              {study && v && <StatusStat label={t('app.status.record')} value={v.cursor}
                                         unit={t('app.status.of_moves', { n: v.moves.length })} />}
              {/* Current viewpoint as state — the sign's meaning is
                  invisible in the numbers (§2 puts it here too). */}
              {study && <StatusStat value={t(pov === 'b' ? 'app.study.black_view' : 'app.study.white_view')} />}
              <StatusStat label={t('app.status.book')}
                          value={t(g.hasBook ? (g.useBook ? 'app.status.book_on' : 'app.status.book_off')
                            : 'app.status.book_none')} />
              <StatusStat label="KUROOBI" value={lv} />
            </>} />


      {ask && (
        <Confirm title={ask.title} body={ask.body} ok={ask.ok} danger={ask.danger}
                 onCancel={() => { ask.done(false); setAsk(null); }}
                 onOk={() => { ask.done(true); setAsk(null); }} />
      )}

      {viewer && (
        <KifuViewer title={viewer.title} kifu={viewer.kifu}
                    parts={viewer.parts} me={ggs.snap?.login}
                    onClose={() => setViewer(null)}
                    // Ask the archive only when the local record fails
                    // (once).
                    onRefetch={viewer.archive && viewer.pending !== viewer.archive
                      ? () => {
                          const id = viewer.archive!;
                          setViewer((cur) => (cur ? { ...cur, kifu: '', pending: id } : cur));
                          void ggsApi.look(id);
                        }
                      : undefined}
                    onStudy={(text) => { setNav('study'); void loadFromText(text); }} />
      )}

      {paste && (
        <PasteKifu onCancel={() => setPaste(false)}
                   onFile={() => void loadFromFile()}
                   onLoad={(t) => void loadFromText(t)} />
      )}

      {/* Settings: same document now, values flow directly (the
          window era went through localStorage + storage events). */}
      {settings && (
        <Overlay onClose={() => setSettings(false)}>
          <Settings prefs={prefs} setPref={setPref} ggs={ggs.snap}
                    initialTab={settingsTab}
                    onClose={() => setSettings(false)} />
        </Overlay>
      )}

      <Toasts items={toasts} onDismiss={(id) => g.dismiss(+id)} />
    </AppFrame>
  );
}

/** Short display names for env vars. Raw variable names stay off
 *  screen. Unlisted vars show their name minus KUROOBI_, so a missing
 *  entry never hides the badge. Built per call: the names are
 *  translated at render. */
const envLabels = (): Record<string, string> => ({
  KUROOBI_NO_RATED: t('app.env.no_rated'),
  KUROOBI_GGS_DEMO: t('app.env.ggs_demo'),
  KUROOBI_GGS_AUTOCONNECT: t('app.env.autoconnect'),
  KUROOBI_GGS_AUTOVIEW: t('app.env.autoview'),
  KUROOBI_GGS_AUTOWATCH: t('app.env.autowatch'),
  KUROOBI_GGS_AUTOLOOK: t('app.env.autolook'),
  KUROOBI_AUTOPLAY: t('app.env.autoplay'),
  KUROOBI_THEME: t('app.env.theme'),
  KUROOBI_LEARN_LOG: t('app.env.learn_log'),
  KUROOBI_KEYCHAIN_SERVICE: t('app.env.keychain'),
  KUROOBI_SESSION_LOCK: t('app.env.session_lock'),
  KUROOBI_WEIGHTS_DIR: t('app.env.weights_dir'),
});

/** Env-var launch badges.
 *
 *  Values show only when they choose behavior: "=1" merely means set
 *  (autoplay both:14 matters, unrated 1 does not). Path values are
 *  omitted too (they fit nowhere); hovering shows name and value in
 *  full.
 *
 *  Lives at the WindowBar's right edge — full width, immune to the
 *  collapse tiers, no rail escape hatch needed. */
function EnvTags({ items }: { items: [string, string][] }) {
  if (!items.length) return null;
  const title = items.map(([k, v]) => `${k}=${v}`).join('\n');
  const names = envLabels();
  return (
    <div className="k-env" title={title}>
      {items.map(([k, v]) => {
        const label = names[k] ?? k.replace(/^KUROOBI_/, '');
        // Values containing / are path overrides; the name suffices.
        const val = v === '1' || v.includes('/') ? '' : v;
        return (
          <span key={k} className="k-env-tag">
            {label}
            {val && <span className="k-env-val">{val}</span>}
          </span>
        );
      })}
    </div>
  );
}

/** Player names for GGF, KUROOBI on its color. GGF is read by other
 *  software, so ASCII only. */
function ggfNames(side: 'black' | 'white' | 'both' | 'off'): [string, string] {
  if (side === 'both') return ['KUROOBI', 'KUROOBI'];
  if (side === 'black') return ['KUROOBI', 'Player'];
  if (side === 'white') return ['Player', 'KUROOBI'];
  return ['Player', 'Player'];
}

/** Compact large numbers so widths stay put. */
const fmtNodes = (n: number): string =>
  n >= 1e9 ? (n / 1e9).toFixed(1) + 'G' : n >= 1e6 ? (n / 1e6).toFixed(1) + 'M'
    : n >= 1e3 ? (n / 1e3).toFixed(0) + 'k' : String(n);

/** Running jobs, always shown under the nav. Local search and GGS
 *  games use separate thread pools, hence separate rows. */
function jobsOf(cpu: ActivityView) {
  const jobs: { label: string; threads?: number; yielded?: boolean }[] = [];
  // cpu.local is a stable token from the backend (calibrating /
  // thinking / pondering / analyzing), translated here.
  if (cpu.local) jobs.push({ label: t('activity.' + cpu.local), threads: cpu.local_threads });
  if (cpu.ggs_match) jobs.push({ label: t('app.jobs.ggs_game'), threads: cpu.ggs_thinking ? cpu.ggs_threads : undefined });
  if (cpu.learn) {
    jobs.push({ label: t('app.jobs.learning', { done: cpu.learn[0], total: cpu.learn[1] }),
                yielded: cpu.learn_paused });
  }
  return jobs;
}
