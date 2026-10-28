import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { PlayDock } from './PlayDock';
import { useGame, type EngineSide } from './state';
import { fmtSecs, useGgs } from './ggs';
import { flipped, usePrefs } from './prefs';
import type { GameView } from './types';
import { api, ggsApi, jsLog, onApp } from './api';
import { useActivity, useEngineSettings, useEngineTurn, useGraph, useHints, useLearnLog, useStartGame, type AskArgs } from './engine';
import { connOf, evalsOf, ggsPlaying, movesOf, navBadges, sqName } from './adapt';
import { AppFrame, BoardFit, Body, Busy, Divider, Dock, Main, Toolbar, WindowBar, useToolbarCompact } from './components/layout';
import { GgsScreen } from './GgsScreens';
import { Confirm, PasteKifu } from './Dialogs';
import { Board, type Cell } from './components/board';
import { EvalGraph, MoveScrub, ScoreRow, StoneDot, srcIsBook, srcLabel } from './components/data';
import { Nav, NavRow, navLocal, ggsNav, Toasts, type NavId } from './components/ggs';
import { Button, Progress, Segmented, Select, Toggle } from './components/primitives';
import { BookDock, BookPane, BookTree, useBookBrowse } from './BookScreen';
import { LearnLog } from './LearnLog';
import { KifuViewer } from './KifuViewer';
import { t, useLang, tErr } from './i18n';
import { SettingsScreen, settingsTitle, useServers, type SettingsCat } from './SettingsScreen';
import { AskSheet, askSheet, useAskSheet } from './AskSheet';
import { onGgsFailure } from './ggsui';
import { markApplied, useApplied } from './applied';
import { Icon } from './components/Icons';
import { GlobalStatus } from './GlobalStatus';


function RolePicker({ value, onChange }: {
  value: EngineSide;
  onChange: (v: EngineSide) => void;
}) {
  if (useToolbarCompact()) {
    return <Select value={value} title={t('app.side.role')} width={116}
                   onChange={(v) => onChange(v as EngineSide)} options={sideChoices()} />;
  }
  return (
    <>
      <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>KUROOBI</span>
      <Segmented value={value} onChange={onChange} options={sides()} />
    </>
  );
}

const sideChoices = (): [string, string][] => [
  ['black', t('app.color.black')],
  ['white', t('app.color.white')],
  ['both', t('app.side.both')],
  ['off', t('app.side.off')],
];

const sides = () => [
  { value: 'black' as const, label: <><StoneDot color="b" />{t('app.color.black')}</> },
  { value: 'white' as const, label: <><StoneDot color="w" />{t('app.color.white')}</> },
  {
    value: 'both' as const,
    label: <><span style={{ display: 'flex', gap: 2 }}>
      <StoneDot color="b" /><StoneDot color="w" />
    </span>{t('app.side.both')}</>,
  },
  { value: 'off' as const, label: t('app.side.off') },
];


function reachable(raw: NavId, conn: ReturnType<typeof connOf>): NavId {
  if (conn === 'online') return raw === 'ggs-login' ? 'ggs-play' : raw;
  return raw.startsWith('ggs-') && raw !== 'ggs-login' ? 'ggs-login' : raw;
}

export function App() {
  useLang();
  const { prefs, set: setPref } = usePrefs();
  const applied = useApplied();
  const g = useGame(prefs.clockSecs, prefs.learnOn);
  const ggs = useGgs();
  const [navRaw, setNavRaw] = useState<NavId>('play');
  const conn = connOf(ggs.snap?.conn);
  const nav = reachable(navRaw, conn);
  const study = nav === 'study';
  const isBook = nav === 'book';
  const isGgs = nav.startsWith('ggs');
  const [bookTab, setBookTab] = useState('book');
  const [dockOpen, setDockOpen] = useState(false);
  const [graphOpen, setGraphOpen] = useState(false);
  const [pov, setPov] = useState<'b' | 'w'>('b');

  const setHasBook = g.setHasBook;
  useEffect(() => {
    const off = onApp('resources-changed', () => {
      void api.hasBook().then(setHasBook).catch(() => { /* engine not started yet */ });
    });
    return () => { void off.then((f) => f()); };
  }, [setHasBook]);

  useEngineSettings(g);
  useEngineTurn(g);
  const cpu = useActivity();
  const [ask, setAsk] = useState<(AskArgs & { done: (ok: boolean) => void }) | null>(null);
  const confirm = useCallback(
    (a: AskArgs) => new Promise<boolean>((done) => setAsk({ ...a, done })),
    [setAsk]);
  // A new game throws the record away (study shows the same one), so a board with moves on it asks first.
  const newGame = useCallback(async () => {
    if ((g.view?.moves.length ?? 0) > 0 && !(await confirm({
      title: t('app.play.new_game_title'), body: t('app.play.new_game_body'), ok: t('app.play.new_game'),
    }))) return;
    await g.newGame();
  }, [g, confirm]);
  const ggsMatch = ggsPlaying(ggs.snap);
  const graph = useGraph(g, ggsMatch, confirm);
  useHints(g, graph.busy);
  const startGame = useStartGame(g, ggsMatch, graph, confirm);

  const chatMsgs = ggs.snap?.chat ?? [];
  const chatSeen = ggs.snap?.chat_seen ?? 0;
  const chatLatest = chatMsgs.length ? Math.max(...chatMsgs.map((m) => m.at)) : 0;
  const chatOpen = nav === 'ggs-chat';
  const chatUnread = chatOpen ? 0 : chatMsgs.filter((m) => m.at > chatSeen).length;

  const markChatRead = useCallback(() => {
    if (chatLatest > chatSeen) void ggsApi.chatSeen(chatLatest);
  }, [chatLatest, chatSeen]);

  useEffect(() => {
    if (chatOpen) markChatRead();
  }, [chatOpen, markChatRead]);

  const setMode = g.setMode;
  // Leaving the game for another screen pauses it, clock and all, like any game one plays alone;
  // 「対局開始」 takes it on from the same position.
  const { playing, stop } = g;
  const setNav = useCallback((id: NavId) => {
    if (navRaw === 'ggs-chat' && id !== 'ggs-chat') markChatRead();
    if (navRaw === 'play' && id !== 'play' && playing) stop();
    setNavRaw(id);
    if (id === 'play' || id === 'study') setMode(id === 'study' ? 'study' : 'vs');
  }, [setMode, navRaw, markChatRead, playing, stop]);

  // Settings shows how much of the book was learned, so it reads the book too.
  const book = useBookBrowse(isBook || nav === 'settings');
  // An import that just ended changed the book: what is on screen is read again.
  const learning = !!cpu?.learn;
  const wasLearning = useRef(learning);
  const reloadBook = book.reload;
  useEffect(() => {
    if (wasLearning.current && !learning) reloadBook();
    wasLearning.current = learning;
  }, [learning, reloadBook]);
  const { items: learnLog, reload: learnLogReload } = useLearnLog(
    isBook && bookTab === 'log', !!cpu?.learn);

  const [viewer, setViewer] = useState<
    { title: string; kifu: string; pending?: string; archive?: string; parts?: string[] } | null
  >(null);
  // Records still being fetched for a viewer that was closed: they are dropped when they arrive.
  const dropped = useRef(new Set<string>());
  const closeViewer = () => {
    if (viewer?.pending && !viewer.kifu) dropped.current.add(viewer.pending);
    setViewer(null);
  };

  const [paste, setPaste] = useState(false);
  const [settingsCat, setSettingsCat] = useState<SettingsCat>('engine');
  const isSettings = nav === 'settings';
  // The right panel exists on the play/study screens and on the book tab when there is a book;
  // the toolbar's panel button and the room kept for the panel follow it.
  const bookDock = isBook && bookTab === 'book' && book.node?.size !== 0;
  const playDock = !isGgs && !isBook && !isSettings;
  // The board's live evals search until stopped and hold the engine, so they stop while the board is
  // off screen (the book and settings read the engine), and do not start again until it is back
  // (the hints effect restarts them). A game's own search and the graph analysis are left running.
  const setBoardShown = g.setBoardShown;
  useEffect(() => {
    setBoardShown(playDock);
    if (!playDock && !g.thinking && !graph.busy) void api.stopSearch().catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playDock]);
  const hasDock = bookDock || playDock;
  const asking = useAskSheet();
  // The sheet only shows while connected; a sheet left open across a disconnect would stay
  // invisible and still hold the keyboard shortcuts, so a lost connection closes it.
  const askOnline = ggs.snap?.conn === 'online';
  useEffect(() => { if (!askOnline) askSheet.close(); }, [askOnline]);
  const { servers } = useServers();

  const started = useRef(false);
  const autoGraph = useRef<(() => void) | null>(null);
  const bookLine = useRef<((kifu: string) => void) | null>(null);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void api.autoplay().then(async (v) => {
      if (!v) return;
      const [who, lv] = v.split(':');
      if (v.endsWith(':nobook')) g.setUseBook(false);
      const mc = v.match(/:clock(\d+)$/);
      if (mc) { setPref('clockSecs', +mc[1]); void api.setClock(+mc[1]); }
      if (who === 'settings') {
        if (lv === 'view' || lv === 'engine') setSettingsCat(lv);
        if (lv === 'ggs') setSettingsCat('ggs-account');
        setNavRaw('settings');
        return;
      }
      if (who === 'overlay') {
        if (lv === 'paste') { setPaste(true); return; }
        if (lv === 'confirm') {
          void confirm({ title: t('app.autoplay.undo_title'),
                         body: t('app.autoplay.undo_body'),
                         ok: t('app.autoplay.undo_ok') });
          return;
        }
        if (lv === 'toast') {
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
      if (who === 'yield') {
        g.setSide('both');
        g.setLevel(0);   // Fastest: end the game early so the import starts
        g.setPlaying(true);
        void (async () => {
          for (let i = 0; i < 240; i++) {
            await new Promise((r) => setTimeout(r, 500));
            const a = await api.activity().catch(() => null);
            if (a?.learn) break;
          }
          await g.newGame();
          g.setUseBook(false);
          g.setLevel(12);
          g.setPlaying(true);
        })();
        return;
      }
      if (who === 'tab') {
        if (v.endsWith(':custom')) { g.setCustom({ depth: 20, solve: 24 }); g.setLevel('custom'); }
        return;
      }
      if (who === 'book') {
        setNavRaw('book');
        if (lv === 'log') setBookTab('log');
        else if (lv) bookLine.current?.(lv);
        return;
      }
      if (who === 'study') {
        await new Promise((r) => setTimeout(r, 500));
        setNavRaw('study');
        g.setMode('study');
        g.setView(await api.loadKifuText(
          'e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2'));
        if (lv === 'hint') {
          // Study shows the evals from the start; the call here would set play's switch.
          g.setView(await api.goto(8));
        }
        if (lv === 'graph') setTimeout(() => autoGraph.current?.(), 400);
        return;
      }
      if (who === 'both') g.setSide('both');
      if (lv !== undefined && Number.isFinite(+lv)) g.setLevel(+lv);
      await g.newGame();
      // newGame set the clock it was made with (the saved one); the requested clock goes on top.
      if (mc) await g.resetClock(+mc[1]);
      g.setPlaying(true);
    }).catch((e) => jsLog('autoplay: ' + e));
  }, []);
  useEffect(() => { autoGraph.current = () => void graph.update(); }, [graph]);
  useEffect(() => { bookLine.current = book.goto; }, [book.goto]);

  const [envOverrides, setEnvOverrides] = useState<[string, string][]>([]);
  useEffect(() => { void api.envOverrides().then(setEnvOverrides).catch(() => {}); }, []);

  useEffect(() => {
    void ggsApi.autoview().then((v) => {
      const to = v.split(':')[0];
      if (to) setNavRaw(('ggs-' + to) as NavId);
    }).catch(() => {});
  }, []);

  const applyLoaded = (v: GameView) => {
    g.setMoveSource({});
    g.setThinkTotal({ black: 0, white: 0 });
    g.setPlaying(false);
    g.setView(v);
  };
  // Loading a record replaces the one play and study share, so a board with a record asks first
  // (the same question from every way in: open in study, load, paste).
  const replaceOk = async () => {
    const n = g.view?.moves.length ?? 0;
    return n === 0 || confirm({
      title: t('app.study.replace_title'), body: t('app.study.replace_body', { n }), ok: t('app.study.replace_ok'),
    });
  };
  const loadFromFile = async () => {
    if (!await replaceOk()) return;
    try {
      const loaded = await api.loadKifu();
      if (loaded) { applyLoaded(loaded); setPaste(false); }   // null = closed without choosing
    } catch (e) { g.say(tErr(e)); }
  };
  const loadFromText = async (text: string, ply?: number) => {
    try {
      applyLoaded(await api.loadKifuText(text));
      setPaste(false);
      if (ply !== undefined) await g.jumpTo(ply);
    } catch (e) { g.say(tErr(e)); }
  };
  // "Open in study" from another screen replaces the record play and study share, so a record asks first.
  const openInStudy = async (text: string, ply?: number): Promise<boolean> => {
    if (!await replaceOk()) return false;
    setNav('study');
    await loadFromText(text, ply);
    return true;
  };

  // A GGS action a person asked for that failed (the screens report it through ggsui).
  const sayFailure = g.say;
  useEffect(() => onGgsFailure((e) => sayFailure(tErr(e))), [sayFailure]);
  const notice = ggs.snap?.notice ?? '';
  const fetched = ggs.snap?.fetched_ggf ?? null;
  useEffect(() => {
    if (!notice) return;
    void (async () => {
      g.say(tErr(notice));
      await ggsApi.ack().catch(() => {});
    })();
  }, [notice]);
  useEffect(() => {
    if (!fetched) return;
    void (async () => {
      // Acknowledged first: opening it may wait on a question.
      await ggsApi.ack().catch(() => {});
      if (dropped.current.delete(fetched.id)) return;
      if (fetched.ggf) {
        setViewer((cur) => (cur && cur.pending === fetched.id
          ? { ...cur, kifu: fetched.ggf, parts: fetched.parts }
          : cur));
        if (!viewer?.pending) await openInStudy(fetched.ggf);
      } else {
        setViewer(null);
        g.say(fetched.error ? tErr(fetched.error) : t('app.ggs.fetch_failed'));
      }
    })();
  }, [fetched]);

  const v = g.view;
  const sign: 1 | -1 = study && pov === 'w' ? -1 : 1;
  const moves = useMemo(
    () => (v ? movesOf(v, g.moveSource, graph.values, sign) : []),
    [v, g.moveSource, graph.values, sign]);
  const evals = g.autoHint ? evalsOf(g.hints, v?.player !== 'white', sign) : undefined;
  const povPoints = useMemo(
    () => (graph.values ?? []).map((p) => (p && sign === -1 ? { ...p, value: -p.value } : p)),
    [graph.values, sign]);
  const blunder = useMemo(() => {
    let best: { at: number; loss: number } | undefined;
    for (const m of moves) if (m.loss && (!best || m.loss > best.loss)) best = { at: m.n, loss: m.loss };
    return best;
  }, [moves]);

  const cur = v && v.cursor > 0 ? moves[v.cursor - 1] : undefined;
  const curMoveMeta = cur && cur.score !== undefined ? (
    <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
      {t('app.study.move_eval')}
      <b style={{
        fontSize: 'var(--fs-3)', fontWeight: 600, color: 'var(--text)',
        fontVariantNumeric: 'tabular-nums',
      }}>{cur.score > 0 ? '+' : ''}{cur.score.toFixed(1)}</b>
      {!!cur.loss && (
        <span style={{ color: 'var(--bad)', fontVariantNumeric: 'tabular-nums' }}>
          ▼{cur.loss.toFixed(1)}
        </span>
      )}
      <span>{cur.pass ? t('app.moves.pass') : cur.move} · {t(cur.color === 'b' ? 'app.color.black' : 'app.color.white')}</span>
      {cur.src && <span style={{ color: srcIsBook(cur.src) ? 'var(--gold)' : 'var(--sub)' }}>{srcLabel(cur.src)}</span>}
    </span>
  ) : undefined;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return;
      if (paste || ask || viewer || asking) return;
      // Any dialog a screen opened itself (a delete question, the server editor) holds the keys too.
      if (document.querySelector('[data-k-overlay]')) return;
      const cmd = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();
      if (cmd && key === ',') { e.preventDefault(); setNav('settings'); return; }
      if (cmd && key === 'b') { e.preventDefault(); setNav('book'); return; }
      // The game shortcuts act on the local board, so only where it is on screen (play and study);
      // elsewhere ⌘Z stays the text fields' own undo.
      if (cmd && key === 'n' && playDock) { e.preventDefault(); if (!g.thinking) void newGame(); return; }
      if (cmd && key === 's' && playDock) {
        e.preventDefault();
        if (v && v.moves.length > 0) {
          void api.saveKifu(...ggfNames(g.side)).catch((err) => g.say(tErr(err)));
        }
        return;
      }
      if (cmd && key === 'o' && playDock) { e.preventDefault(); setPaste(true); return; }
      if (cmd && key === 'z' && playDock) {
        e.preventDefault();
        if (!g.thinking && v && v.move_count > 0) void g.undo();
        return;
      }
      if (isBook) {
        // The arrows walk the book; the learning log tab has nothing for them to move.
        if (bookTab !== 'book') return;
        if (e.key === 'ArrowLeft') { e.preventDefault(); book.back(); }
        if (e.key === 'ArrowUp') { e.preventDefault(); book.reset(); }
        if (e.key === 'ArrowRight' && book.node?.moves.length) {
          e.preventDefault();
          book.push(book.node.moves[0].pos);
        }
        return;
      }
      if (!study || !v) return;
      const step = e.shiftKey ? 10 : 1;
      if (e.key === 'ArrowLeft') { e.preventDefault(); void g.jumpTo(cmd ? 0 : Math.max(0, v.cursor - step)); }
      if (e.key === 'ArrowRight') {
        e.preventDefault();
        void g.jumpTo(cmd ? v.moves.length : Math.min(v.moves.length, v.cursor + step));
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [setNav, paste, ask, viewer, asking, isBook, bookTab, isGgs, playDock, study, book, g, v, newGame]);

  // Studying a record, the record's own next move just steps forward; another move drops the rest, so it asks.
  // On KUROOBI's turn the board takes no move, so it shows none under the pointer either.
  const kuroobiTurn = !study && !!v && !v.over && g.engineSides().includes(v.player);
  const playAt = async (sq: number) => {
    if (study && v && v.cursor < v.moves.length) {
      if (v.moves[v.cursor] === sq) {
        let n = v.cursor + 1;
        while (n < v.moves.length && v.moves[n] == null) n++;   // a forced pass comes with the move
        await g.jumpTo(n);
        return;
      }
      if (!(await confirm({
        title: t('app.study.branch_title'),
        body: t('app.study.branch_body', { n: v.cursor + 1, m: v.moves.length - v.cursor }),
        ok: t('app.study.branch_ok'),
      }))) return;
    }
    // A move before 「対局開始」 starts the game and KUROOBI answers it; with no side for KUROOBI it is just a move.
    if (!study && v && !v.over && !g.playing && g.side !== 'off') {
      await g.play(sq);
      await startGame();
      return;
    }
    await g.play(sq);
  };

  const over = v?.over ?? false;
  const result = v?.over
    ? t(v.black === v.white ? 'app.result.draw'
        : v.black > v.white ? 'app.result.black_wins' : 'app.result.white_wins')
    : undefined;
  const anyThink = g.thinkTotal.black > 0 || g.thinkTotal.white > 0;
  const clockLabel = (c: 'b' | 'w') => {
    if (g.clockSecs) {
      const v = c === 'b' ? g.clock?.black : g.clock?.white;
      return v === undefined ? undefined : fmtSecs(v);
    }
    return !study && anyThink ? fmtSecs(c === 'b' ? g.thinkTotal.black : g.thinkTotal.white) : undefined;
  };


  const nodes = g.stat && g.stat.nodes > 0 ? g.stat.nodes : 0;
  const nps = nodes && g.stat && g.stat.secs > 0 ? nodes / g.stat.secs : 0;
  const lv = g.level === 'custom' ? t('app.level.custom') : t('app.level.preset', { n: g.level + 1 });
  // Playing to a clock, KUROOBI's strength is the clock.
  const playLv = prefs.clockSecs > 0 ? t('dock.strength.minutes', { n: prefs.clockSecs / 60 }) : lv;
  const screenTitle = isSettings ? t('settings.title')
    : [...navLocal(), ...ggsNav(conn)].find((i) => i.id === nav)?.label ?? 'KUROOBI';
  const screenSub = isGgs || isSettings ? undefined
    : isBook ? undefined
    // Studying, KUROOBI plays no side; its level is the analysis strength.
    : study ? t('app.study.analysis_level', { lv })
    : `${playLv} · ${t(g.side === 'both' ? 'app.side.both'
        : g.side === 'off' ? 'app.side.none_assigned'
        : g.side === 'black' ? 'app.color.black' : 'app.color.white')}`;

  const sideColor = g.side === 'black' ? 'white' : g.side === 'white' ? 'black' : '';

  return (
    <AppFrame>
      <WindowBar title={screenTitle} sub={screenSub} right={<EnvTags items={envOverrides} />} />

      <Body>

      <Nav items={navLocal()} ggsItems={ggsNav(conn, navBadges(ggs.snap, chatUnread))} conn={conn}
           active={nav} onSelect={setNav}
           footer={<NavRow item={{ id: 'settings', label: t('ggs.nav.settings'), icon: 'prefs', hint: '⌘,' }}
                           active={isSettings} onSelect={setNav} />} />

      <Main inset={dockOpen && hasDock}>
      {isSettings && (
        <Toolbar>
          <span style={{ fontSize: 'var(--fs-3)', fontWeight: 600, whiteSpace: 'nowrap' }}>{settingsTitle(settingsCat)}</span>
          {applied && (
            <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-1)',
                           fontSize: 'var(--fs-6)', color: 'var(--ok)', whiteSpace: 'nowrap' }}>
              <Icon name="check" size={14} />{t('settings.applied')}
            </span>
          )}
        </Toolbar>
      )}
      {!isGgs && !isSettings && <Toolbar
          dock={hasDock ? { open: dockOpen, onToggle: () => setDockOpen(o => !o) } : undefined}
          graph={study && !isGgs && !isBook
            ? { open: graphOpen, onToggle: () => setGraphOpen(o => !o) } : undefined}
          aux={isBook
            ? (cpu?.learn
              ? <Busy>{t('app.book.importing')}<span style={{ color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                    {t('app.book.import_count', { done: cpu.learn[0].toLocaleString(),
                                                  total: cpu.learn[1].toLocaleString() })}
                  </span>
                </Busy>
              : undefined)
            : undefined}>
          {isBook ? (
            <>
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
          ) : study ? (
            <>
              <Button variant="primary" disabled={graph.busy || !v?.moves.length}
                      onClick={() => void graph.update()}>{t('app.study.analyze')}</Button>
              <Toggle checked={g.autoHint} onChange={g.setAutoHint} label={t('app.play.show_evals')} />
              <Divider />
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
                      // Nothing to start when KUROOBI plays no side and there is no clock to run.
                      disabled={!g.playing && (over || g.thinking || (g.side === 'off' && !prefs.clockSecs))}
                      onClick={startGame}>
                {t(g.playing ? 'app.play.stop' : 'app.play.start')}
              </Button>
              <Button title="⌘N" disabled={g.thinking} onClick={() => void newGame()}>{t('app.play.new_game')}</Button>
              <Button title="⌘Z" disabled={g.thinking || !v || v.move_count === 0}
                      onClick={() => void g.undo()}>{t('app.play.undo')}</Button>
              <Divider />
              <RolePicker value={g.side} onChange={g.setSide} />
              <Divider />
              <Toggle checked={g.autoHint} onChange={g.setAutoHint} label={t('app.play.show_evals')} />
            </>
          )}
      </Toolbar>}

        {isSettings ? <SettingsScreen cat={settingsCat} onCat={setSettingsCat} prefs={prefs}
                                       setPref={(k, v) => { setPref(k, v); markApplied(); }} snap={ggs.snap}
                                       onLogin={() => setNav('ggs-login')}
                                       learn={{ on: g.learnOn, set: (on) => { setPref('learnOn', on); markApplied(); },
                                                stored: book.node?.size, learned: book.node?.learned_size,
                                                onLog: () => { setBookTab('log'); setNav('book'); } }} />
         : isGgs ? <GgsScreen nav={nav} snap={ggs.snap} onNav={setNav} prefs={prefs}
                       onKifu={(title, kifu, archive) => {
                         if (archive) {
                           setViewer({ title, kifu: '', pending: archive, archive });
                           void ggsApi.look(archive);
                         } else {
                           setViewer({ title, kifu });
                         }
                       }} />
         : isBook ? (
          bookTab === 'book' ? (
            <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
              {book.node?.size !== 0 && (
                <BookTree b={book} decimals={prefs.decimals}
                          onStudy={(kifu) => void openInStudy(kifu)} />
              )}
              <BookPane b={book} coords={prefs.coords} grain={prefs.grain}
                        flip={flipped(prefs.facing, '')} onSettings={() => { setSettingsCat('engine'); setNav('settings'); }} />
            </div>
          ) : (
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
                onOpen={(e, ply) => void openInStudy(e.start ? e.start + '\n' + e.kifu : e.kifu, ply)} />
            </div>
          )
        ) : (
        <div style={{
          flex: 1, minHeight: 'calc(200px + var(--h-bar))',
          display: 'flex', flexDirection: 'column',
        }}>
          <BoardFit minHeight={200} padding="var(--sp-2) var(--sp-4)">
            {v && <Board cells={v.cells as Cell[]} legal={v.legal} last={v.last} evals={evals}
                         coords={prefs.coords} grain={prefs.grain}
                         flip={flipped(prefs.facing, study ? '' : sideColor)}
                         disabled={g.thinking || kuroobiTurn}
                         onPlay={(sq) => void playAt(sq)} />}
          </BoardFit>
          <ScoreRow black={v?.black ?? 2} white={v?.white ?? 2}
                    turn={!v || v.over ? undefined : v.player === 'black' ? 'b' : 'w'}
                    meta={study ? curMoveMeta : result}
                    blackClock={clockLabel('b')} whiteClock={clockLabel('w')} />
        </div>
        )}

        {study && !isGgs && !isBook && v && (
          <MoveScrub plies={v.moves.length} cursor={v.cursor} blunder={blunder}
                     onSeek={(n) => void g.jumpTo(n)} />
        )}

        {study && !isGgs && !isBook && (
          <EvalGraph points={povPoints} plies={v?.moves.length} cursor={v?.cursor}
                     blunder={blunder} busy={graph.busy} onJump={(n) => void g.jumpTo(n)}
                     open={graphOpen} pov={pov}
                     moveName={(n) => { if (!v || n < 1 || n > v.moves.length) return undefined; const m = v.moves[n - 1]; return m == null ? t('app.moves.pass') : sqName(m); }}
                     extra={<>
                       {graph.prog && (
                         <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
                           {t('app.status.analyzing')} <b style={{ color: 'var(--text)' }}>{graph.prog.done}</b>/{graph.prog.total}
                           <span style={{ width: 72 }}>
                             <Progress value={graph.prog.total > 0 ? graph.prog.done / graph.prog.total : 0} />
                           </span>
                         </span>
                       )}
                       {graph.busy && (
                         <Button variant="danger" onClick={() => graph.stop()}>{t('app.study.stop_analysis')}</Button>
                       )}
                     </>} />
        )}

      </Main>

      {bookDock && (
        <Dock tabs={[{ id: 'book', label: t('app.book.tab_book') }]} active="book" open={dockOpen}>
          <BookDock b={book} decimals={prefs.decimals} />
        </Dock>
      )}

      {playDock && (
        <PlayDock g={g} prefs={prefs} setClockSecs={(secs) => { setPref('clockSecs', secs); void g.resetClock(secs); }}
                  open={dockOpen}
                  onPaste={() => setPaste(true)} onLoadFile={loadFromFile}
                  study={study} moves={moves} ggfNames={() => ggfNames(g.side)} />
      )}

      </Body>

        <GlobalStatus snap={ggs.snap} cpu={cpu} analyzing={graph.busy ? graph.prog : null} thinking={g.thinking}
                      thinkSecs={g.thinkSecs} nodes={nodes} nps={nps} servers={servers} />



      {viewer && (
        <KifuViewer title={viewer.title} kifu={viewer.kifu}
                    parts={viewer.parts}
                    onClose={closeViewer}
                    onRefetch={viewer.archive && viewer.pending !== viewer.archive
                      ? () => {
                          const id = viewer.archive!;
                          setViewer((cur) => (cur ? { ...cur, kifu: '', pending: id } : cur));
                          void ggsApi.look(id);
                        }
                      : undefined}
                    onStudy={(text) => openInStudy(text)} />
      )}

      {paste && (
        <PasteKifu onCancel={() => setPaste(false)}
                   onFile={() => void loadFromFile()}
                   onLoad={(text) => void (async () => { if (await replaceOk()) await loadFromText(text); })()} />
      )}

      {asking && ggs.snap?.conn === 'online' && <AskSheet snap={ggs.snap} init={asking} />}

      {/* Last, so a question asked over the viewer or the paste dialog sits on top of it. */}
      {ask && (
        <Confirm title={ask.title} body={ask.body} ok={ask.ok} danger={ask.danger}
                 onCancel={() => { ask.done(false); setAsk(null); }}
                 onOk={() => { ask.done(true); setAsk(null); }} />
      )}

      <Toasts items={g.toasts} onDismiss={g.dismiss} />
    </AppFrame>
  );
}

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
  KUROOBI_SESSION_LOCK: t('app.env.session_lock'),
  KUROOBI_WEIGHTS_DIR: t('app.env.weights_dir'),
});

function EnvTags({ items }: { items: [string, string][] }) {
  if (!items.length) return null;
  const title = items.map(([k, v]) => `${k}=${v}`).join('\n');
  const names = envLabels();
  return (
    <div className="k-env" title={title}>
      {items.map(([k, v]) => {
        const label = names[k] ?? k.replace(/^KUROOBI_/, '');
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

function ggfNames(side: 'black' | 'white' | 'both' | 'off'): [string, string] {
  const me = t('kifu.player');
  if (side === 'both') return ['KUROOBI', 'KUROOBI'];
  if (side === 'black') return ['KUROOBI', me];
  if (side === 'white') return [me, 'KUROOBI'];
  return [me, me];
}

