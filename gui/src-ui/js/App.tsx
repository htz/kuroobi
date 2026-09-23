import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { PlayDock } from './PlayDock';
import { useGame, type EngineSide } from './state';
import { useGgs } from './ggs';
import { flipped, usePrefs } from './prefs';
import type { GameView } from './types';
import { api, ggsApi, jsLog, onApp, type ActivityView } from './api';
import { useActivity, useEngineSettings, useEngineTurn, useGraph, useHints, useLearnLog, useStartGame, type AskArgs } from './engine';
import { fmtSecs } from './ggs';
import { cellsOf, connOf, evalsOf, ggsPlaying, movesOf, navBadges, sqName } from './adapt';
import { AppFrame, Body, BottomPanel, Busy, Divider, Dock, Main, Overlay, StatusBar, StatusStat, Toolbar, WindowBar, useToolbarCompact } from './components/layout';
import { GgsChat, GgsConsole, GgsScreen } from './GgsScreens';
import { Confirm, PasteKifu, Settings } from './Dialogs';
import { Board } from './components/board';
import { EvalGraph, MoveScrub, ScoreRow, StoneDot, srcIsBook, srcLabel } from './components/data';
import { GgsStatus, JobList, Meter, Nav, navLocal, StatusChip, ggsNav, Toasts, type NavId, type Toast } from './components/ggs';
import { Button, Progress, Segmented, Select, Toggle } from './components/primitives';
import { Icon } from './components/Icons';
import { BookDock, BookPane, BookTree, useBookBrowse } from './BookScreen';
import { LearnLog } from './LearnLog';
import { KifuViewer } from './KifuViewer';
import { LEVELS } from './state';
import { t, useLang, tErr } from './i18n';


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
  const g = useGame(prefs.clockSecs);
  const ggs = useGgs();
  const [navRaw, setNavRaw] = useState<NavId>('play');
  const conn = connOf(ggs.snap?.conn);
  const nav = reachable(navRaw, conn);
  const study = nav === 'study';
  const isBook = nav === 'book';
  const isGgs = nav.startsWith('ggs');
  const [tab, setTab] = useState('record');
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
  useHints(g);
  useEngineTurn(g);
  const cpu = useActivity();
  const [ask, setAsk] = useState<(AskArgs & { done: (ok: boolean) => void }) | null>(null);
  const confirm = useCallback(
    (a: AskArgs) => new Promise<boolean>((done) => setAsk({ ...a, done })),
    [setAsk]);
  const ggsMatch = ggsPlaying(ggs.snap);
  const graph = useGraph(g, ggsMatch, confirm);
  const startGame = useStartGame(g, ggsMatch, graph, confirm);

  const [panel, setPanel] = useState<'' | 'chat' | 'console'>('');

  const chatMsgs = ggs.snap?.chat ?? [];
  const chatSeen = ggs.snap?.chat_seen ?? 0;
  const chatLatest = chatMsgs.length ? Math.max(...chatMsgs.map((m) => m.at)) : 0;
  const chatOpen = nav === 'ggs-chat' || panel === 'chat';
  const chatUnread = chatOpen ? 0 : chatMsgs.filter((m) => m.at > chatSeen).length;

  const markChatRead = useCallback(() => {
    if (chatLatest > chatSeen) void ggsApi.chatSeen(chatLatest);
  }, [chatLatest, chatSeen]);

  useEffect(() => {
    if (chatOpen) markChatRead();
  }, [chatOpen, markChatRead]);

  const showPanel = useCallback((next: '' | 'chat' | 'console') => {
    setPanel((cur) => {
      if (cur === 'chat' && next !== 'chat') markChatRead();
      return cur === next ? '' : next;
    });
  }, [markChatRead]);

  const setMode = g.setMode;
  const setNav = useCallback((id: NavId) => {
    if (navRaw === 'ggs-chat' && id !== 'ggs-chat') markChatRead();
    setNavRaw(id);
    if (id === 'play' || id === 'study') setMode(id === 'study' ? 'study' : 'vs');
  }, [setMode, navRaw, markChatRead]);

  const book = useBookBrowse(isBook || tab === 'learn');
  const { items: learnLog, reload: learnLogReload } = useLearnLog(
    isBook && bookTab === 'log', !!cpu?.learn);

  const [viewer, setViewer] = useState<
    { title: string; kifu: string; pending?: string; archive?: string; parts?: string[] } | null
  >(null);

  const [paste, setPaste] = useState(false);
  const [settings, setSettings] = useState(false);
  const [settingsTab, setSettingsTab] = useState<'engine' | 'view' | 'ggs'>('engine');

  const started = useRef(false);
  const autoGraph = useRef<(() => void) | null>(null);
  const bookLine = useRef<((kifu: string) => void) | null>(null);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void api.autoplay().then(async (v) => {
      if (!v) return;
      const [who, lv, extraRaw] = v.split(':');
      const extra =
        extraRaw === 'nobook' || /^clock\d+$/.test(extraRaw ?? '') ? undefined : extraRaw;
      if (v.endsWith(':nobook')) g.setUseBook(false);
      const mc = v.match(/:clock(\d+)$/);
      if (mc) { setPref('clockSecs', +mc[1]); void api.setClock(+mc[1]); }
      if (who === 'settings') {
        if (lv === 'ggs' || lv === 'view' || lv === 'engine') setSettingsTab(lv);
        setSettings(true);
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
          await g.newGame();
          g.setUseBook(false);
          g.setLevel(12);
          g.setPlaying(true);
        })();
        return;
      }
      if (who === 'tab') {
        if (lv) setTab(lv);
        if (v.endsWith(':custom')) { g.setCustom({ depth: 20, solve: 24, band: 4 }); g.setLevel('custom'); }
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
          g.setView(await api.goto(8));
          g.setAutoHint(true);
        }
        if (lv === 'graph') setTimeout(() => autoGraph.current?.(), 400);
        if (extra) setTab(extra);
        return;
      }
      if (who === 'both') g.setSide('both');
      if (lv !== undefined && Number.isFinite(+lv)) g.setLevel(+lv);
      await g.newGame();
      if (extra) setTab(extra);
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
  const loadFromFile = async () => {
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
      if (fetched.ggf) {
        setViewer((cur) => (cur && cur.pending === fetched.id
          ? { ...cur, kifu: fetched.ggf, parts: fetched.parts }
          : cur));
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
      if (paste || ask || viewer || settings) return;
      const cmd = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();
      if (cmd && key === ',') { e.preventDefault(); setSettings(true); return; }
      if (cmd && key === 'b') { e.preventDefault(); setNav('book'); return; }
      if (cmd && key === 'n') { e.preventDefault(); if (!g.thinking) void g.newGame(); return; }
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
      if (isBook) {
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
  }, [setNav, paste, ask, viewer, settings, isBook, isGgs, study, book, g, v]);

  const toasts: Toast[] = g.toasts.map(x => ({ id: String(x.id), tone: x.tone, text: x.text }));

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
  const lv = g.level === 'custom' ? t('app.level.custom') : LEVELS[g.level].name;
  const screenTitle =
    [...navLocal(), ...ggsNav(conn)].find((i) => i.id === nav)?.label ?? 'KUROOBI';
  const screenSub = isGgs
    ? (conn === 'online' ? ggs.snap?.login : undefined)
    : isBook ? undefined
    : `${lv} · ${t(g.side === 'both' ? 'app.side.both'
        : g.side === 'off' ? 'app.side.none_assigned'
        : g.side === 'black' ? 'app.color.black' : 'app.color.white')}`;

  const sideColor = g.side === 'black' ? 'white' : g.side === 'white' ? 'black' : '';

  return (
    <AppFrame>
      <WindowBar title={screenTitle} sub={screenSub} right={<EnvTags items={envOverrides} />} />

      <Body>

      <Nav items={navLocal()} ggsItems={ggsNav(conn, navBadges(ggs.snap, chatUnread))} conn={conn}
           active={nav} onSelect={setNav}
           footer={<>
             {cpu && <>
             <Meter icon="cpu" label="CPU" value={Math.round(cpu.cpu)} unit="%"
                    ratio={cpu.cpu / (cpu.cores * 100)} />
             <Meter icon="memory" label={t('app.meter.memory')} value={(cpu.mem / 1e9).toFixed(1)} unit={'\u00a0GB'}
                    ratio={cpu.mem_total > 0 ? cpu.mem / cpu.mem_total : 0} />
             <JobList jobs={jobsOf(cpu)} />
             </>}
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
              <Button title="⌘N" disabled={g.thinking} onClick={() => void g.newGame()}>{t('app.play.new_game')}</Button>
              <Button title="⌘Z" disabled={g.thinking || !v || v.move_count === 0}
                      onClick={() => void g.undo()}>{t('app.play.undo')}</Button>
              <Divider />
              <RolePicker value={g.side} onChange={g.setSide} />
              <Divider />
              <Toggle checked={g.autoHint} onChange={g.setAutoHint} label={t('app.play.show_evals')} />
            </>
          )}
      </Toolbar>

        {isGgs ? <GgsScreen nav={nav} snap={ggs.snap} onNav={setNav} prefs={prefs}
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
                          onStudy={(kifu) => { setNav('study'); void loadFromText(kifu); }} />
              )}
              <BookPane b={book} coords={prefs.coords} grain={prefs.grain}
                        flip={flipped(prefs.facing, '')} onSettings={() => setSettings(true)} />
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
                onOpen={(e, ply) => {
                  setNav('study');
                  void loadFromText(e.start ? e.start + '\n' + e.kifu : e.kifu, ply);
                }} />
            </div>
          )
        ) : (
        <div style={{
          flex: 1, minHeight: 'calc(200px + var(--h-bar))',
          display: 'flex', flexDirection: 'column',
        }}>
          <div style={{
            flex: 1, minHeight: 200, display: 'grid', placeItems: 'center',
            gridTemplateRows: 'minmax(0, 1fr)', gridTemplateColumns: 'minmax(0, 1fr)',
            padding: 'var(--sp-2) var(--sp-4)',
          }}>
              <div style={{ height: '100%', maxHeight: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
              {v && <Board cells={cellsOf(v)} legal={v.legal} last={v.last} evals={evals}
                           coords={prefs.coords} grain={prefs.grain}
                           flip={flipped(prefs.facing, study ? '' : sideColor)}
                           disabled={g.thinking}
                           onPlay={(sq) => void g.play(sq)} />}
            </div>
          </div>
          <ScoreRow black={v?.black ?? 2} white={v?.white ?? 2}
                    turn={!v || v.over ? undefined : v.player === 'black' ? 'b' : 'w'}
                    meta={study ? curMoveMeta : result}
                    blackClock={clockLabel('b')} whiteClock={clockLabel('w')} />
        </div>
        )}

        {isGgs && panel && ggs.snap && (
          <BottomPanel
            tabs={[{ id: 'chat', label: t('app.panel.chat'), unread: panel === 'chat' ? 0 : chatUnread },
                   { id: 'console', label: t('app.panel.console') }]}
            active={panel} onTab={(id) => showPanel(id as 'chat' | 'console')}
            onClose={() => showPanel('')}>
            {panel === 'chat' ? <GgsChat snap={ggs.snap} /> : <GgsConsole snap={ggs.snap} />}
          </BottomPanel>
        )}

        {study && !isGgs && !isBook && v && (
          <MoveScrub plies={v.moves.length} cursor={v.cursor} blunder={blunder}
                     onSeek={(n) => void g.jumpTo(n)} />
        )}

        {study && !isGgs && !isBook && (
          <EvalGraph points={povPoints} plies={v?.moves.length} cursor={v?.cursor}
                     blunder={blunder} busy={graph.busy} onJump={(n) => void g.jumpTo(n)}
                     open={graphOpen} pov={pov}
                     moveName={(n) => { const m = v?.moves[n - 1]; return m == null ? undefined : sqName(m); }}
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

      {isBook && bookTab === 'book' && book.node?.size !== 0 && (
        <Dock tabs={[t('app.book.tab_book')]} active={t('app.book.tab_book')} open={dockOpen}>
          <BookDock b={book} decimals={prefs.decimals} />
        </Dock>
      )}

      {!isGgs && !isBook && (
        <PlayDock g={g} book={book} cpu={cpu} prefs={prefs} tab={tab} onTab={setTab}
                  open={dockOpen} onNav={setNav} onBookTab={setBookTab}
                  onPaste={() => setPaste(true)} onLoadFile={loadFromFile}
                  study={study} moves={moves} ggfNames={() => ggfNames(g.side)} />
      )}

      </Body>

        <StatusBar
          left={<>
            {graph.busy && (
              <Busy>{t('app.status.analyzing')}</Busy>
            )}
            {g.thinking && (
              <Busy>{t('app.status.thinking')}</Busy>
            )}
            {g.thinking && <StatusStat value={g.thinkSecs.toFixed(1)} unit="s" />}
            {nodes > 0 && <StatusStat label="nodes" value={fmtNodes(nodes)} />}
            {nps > 0 && <StatusStat label="nps" value={(nps / 1e6).toFixed(1)} unit="Mnps" />}
          </>}
          right={isGgs
            ? <>
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
              <StatusStat label={t('app.status.stored_positions')}
                          value={book.node ? book.node.size.toLocaleString() : '—'} />
              <StatusStat label={t('app.status.of_which_learned')}
                          value={book.node ? book.node.learned_size.toLocaleString() : '—'} />
            </>
            : <>
              {study && v && <StatusStat label={t('app.status.record')} value={v.cursor}
                                         unit={t('app.status.of_moves', { n: v.moves.length })} />}
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
  if (side === 'both') return ['KUROOBI', 'KUROOBI'];
  if (side === 'black') return ['KUROOBI', 'Player'];
  if (side === 'white') return ['Player', 'KUROOBI'];
  return ['Player', 'Player'];
}

const fmtNodes = (n: number): string =>
  n >= 1e9 ? (n / 1e9).toFixed(1) + 'G' : n >= 1e6 ? (n / 1e6).toFixed(1) + 'M'
    : n >= 1e3 ? (n / 1e3).toFixed(0) + 'k' : String(n);

function jobsOf(cpu: ActivityView) {
  const jobs: { label: string; threads?: number; yielded?: boolean }[] = [];
  if (cpu.local)
    jobs.push({
      label: t('activity.' + cpu.local),
      threads: cpu.local === 'loading' ? undefined : cpu.local_threads,
    });
  if (cpu.ggs_match) jobs.push({ label: t('app.jobs.ggs_game'), threads: cpu.ggs_thinking ? cpu.ggs_threads : undefined });
  if (cpu.learn) {
    jobs.push({ label: t('app.jobs.learning', { done: cpu.learn[0], total: cpu.learn[1] }),
                yielded: cpu.learn_paused });
  }
  return jobs;
}
