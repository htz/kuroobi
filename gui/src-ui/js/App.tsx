import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useGame } from './state';
import { useGgs } from './ggs';
import { flipped, usePrefs } from './prefs';
import type { GameView } from './types';
import { api, ggsApi, jsLog, type ActivityView } from './api';
import { useActivity, useEngineSettings, useEngineTurn, useGraph, useHints, useLearnLog, useStartGame } from './engine';
import { fmtSecs } from './ggs';
import { cellsOf, connOf, evalsOf, ggsPlaying, movesOf, navBadges, sqName } from './adapt';
import { AppFrame, Body, BottomPanel, Dock, Main, Section, StatusBar, StatusStat, Toolbar } from './components/layout';
import { GgsChat, GgsConsole, GgsScreen } from './GgsScreens';
import { Confirm, PasteKifu, Settings } from './Dialogs';
import { Board } from './components/board';
import { EvalGraph, KifuTable, MoveScrub, PlayerRow } from './components/data';
import { JobList, Meter, Nav, NAV_LOCAL, StatusChip, ggsNav, Toasts, type NavId, type Toast } from './components/ggs';
import { Button, Progress, Segmented, Toggle } from './components/primitives';
import { Icon } from './components/Icons';
import { Strength } from './components/strength';
import { BookDock, BookPane, useBookBrowse } from './BookScreen';
import { LearnLog } from './LearnLog';
import { KifuViewer } from './KifuViewer';
import { LEVELS } from './state';

/* 対局と検討の画面。
 *
 * エンジンとのやりとりは engine.ts が持つ (旧 App と同じものを使う)。
 * ここがやるのは「状態を画面の形にして並べる」だけ。
 */

const SIDES = [
  { value: 'black' as const, label: '黒' },
  { value: 'white' as const, label: '白' },
  { value: 'both' as const, label: '両方' },
  { value: 'off' as const, label: 'なし' },
];

export function App() {
  const g = useGame();
  const ggs = useGgs();
  const { prefs, set: setPref } = usePrefs();
  const [nav, setNavRaw] = useState<NavId>('play');
  const study = nav === 'study';
  const isBook = nav === 'book';
  const isGgs = nav.startsWith('ggs');
  const [tab, setTab] = useState('棋譜');
  const [dockOpen, setDockOpen] = useState(false);

  useEngineSettings(g);
  useHints(g);
  useEngineTurn(g);
  const cpu = useActivity();
  // engine.ts は画面を持たないので、確認はこちらが出して答えだけ返す
  const [ask, setAsk] = useState<{ msg: string; done: (ok: boolean) => void } | null>(null);
  const confirm = useCallback(
    (msg: string) => new Promise<boolean>((done) => setAsk({ msg, done })),
    []);
  // GGS 対局は最優先。走っている間はローカル対局も分析も断る
  const ggsMatch = ggsPlaying(ggs.snap);
  const graph = useGraph(g, ggsMatch, confirm);
  const startGame = useStartGame(g, ggsMatch, graph, confirm);

  // チャットの未読数。開いている間は 0 で、離れるときに既読位置を進める
  /* GGS の対局中だけ、下の帯の右端からチャットとコンソールを開けるようにする。
   *
   * 行き先としては左の並びにもあるが、**対局中に離れたくない**。盤を見た
   * まま 1 枚のパネルで済ませる (デザイン規則 12)。 */
  const [panel, setPanel] = useState<'' | 'chat' | 'console'>('');

  const chatTotal = ggs.snap?.chat.length ?? 0;
  const [chatSeen, setChatSeen] = useState(0);
  // 下の板でチャットを開いている間も「読んでいる」扱いにする。
  // 行き先として開いたときと同じにしないと、板を開けたまま未読が増える
  const chatOpen = nav === 'ggs-chat' || panel === 'chat';
  const chatUnread = chatOpen ? 0 : Math.max(0, chatTotal - chatSeen);

  /** 下の板を切り替える。チャットから離れるときに既読位置を進める。 */
  const showPanel = useCallback((next: '' | 'chat' | 'console') => {
    setPanel((cur) => {
      if (cur === 'chat' && next !== 'chat') setChatSeen(chatTotal);
      return cur === next ? '' : next;
    });
  }, [chatTotal]);

  // 行き先とエンジンのモードは同じもの。ずれると検討中に打たれる
  const setMode = g.setMode;
  const setNav = useCallback((id: NavId) => {
    // チャットを離れるときに既読位置を進める。開いている間は 0 のままなので、
    // 離れた後に届いたぶんだけが未読として数えられる
    if (nav === 'ggs-chat' && id !== 'ggs-chat') setChatSeen(chatTotal);
    setNavRaw(id);
    if (id === 'play' || id === 'study') setMode(id === 'study' ? 'study' : 'vs');
  }, [setMode, nav, chatTotal]);

  const conn = connOf(ggs.snap?.conn);

  const book = useBookBrowse(isBook);
  const { items: learnLog, reload: learnLogReload } = useLearnLog(
    tab === '学習' && !isGgs && !isBook, !!cpu?.learn);

  /* 棋譜ビューア (規則 71)。`pending` はアーカイブ番号 — 手元に棋譜が
   * 無い対局は覆いを先に開き、届いたら中身を差し込む。 */
  const [viewer, setViewer] = useState<{ title: string; kifu: string; pending?: string } | null>(null);

  const [paste, setPaste] = useState(false);
  const [settings, setSettings] = useState(false);

  /* 動作確認用 (KUROOBI_AUTOPLAY=both:11 のように指定する)。
   * この repo は画面の確認を「起動して撮る」でやるので、その入口を残す。
   * "study" なら棋譜を読んで検討を開き、"settings" なら設定を開く。 */
  const started = useRef(false);
  // "study:graph" 用。graph.update はこの effect より後ろで作られるので
  // ref 越しに渡す (依存に入れると毎回走り直す)
  const autoGraph = useRef<(() => void) | null>(null);
  const bookLine = useRef<((kifu: string) => void) | null>(null);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void api.autoplay().then(async (v) => {
      if (!v) return;
      const [who, lv] = v.split(':');
      if (who === 'settings') { setSettings(true); return; }
      // "tab:学習" のようにドックの見出しを指定する (撮るためだけの入口)
      if (who === 'tab') { if (lv) setTab(lv); return; }
      // "book:f5d6" のように手順を渡すと、その節まで辿った状態で開く
      if (who === 'book') { setNavRaw('book'); if (lv) bookLine.current?.(lv); return; }
      if (who === 'study') {
        // 起動時の状態取得と前後すると初期局面で上書きされるので一拍おく
        await new Promise((r) => setTimeout(r, 500));
        setNavRaw('study');
        g.setMode('study');
        g.setView(await api.loadKifuText(
          'e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2'));
        // "study:hint" は序盤 (定石内) の局面で評価値表示まで自動で入れる
        if (lv === 'hint') {
          g.setView(await api.goto(8));
          g.setAutoHint(true);
        }
        // "study:graph" は分析まで始める (進み具合の見た目を確かめるため)
        if (lv === 'graph') setTimeout(() => autoGraph.current?.(), 400);
        return;
      }
      if (who === 'both') g.setSide('both');
      if (lv !== undefined && Number.isFinite(+lv)) g.setLevel(+lv);
      g.setPlaying(true);
    }).catch((e) => jsLog('autoplay: ' + e));
    // g は毎描画で作り直されるので依存に入れない (started で 1 度だけに絞っている)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => { autoGraph.current = () => void graph.update(); }, [graph]);
  useEffect(() => { bookLine.current = book.goto; }, [book.goto]);

  /* 画面確認用 (KUROOBI_GGS_AUTOVIEW=players のように指定する)。
   * GGS の画面は開くまで描かれないので、撮るには行き先を指定する経路が要る。 */
  useEffect(() => {
    void ggsApi.autoview().then((v) => {
      if (v) setNavRaw(('ggs-' + v) as NavId);
    }).catch(() => {});
  }, []);

  /** 読み込んだら手順の記録も消す (前の対局の評価が残ると嘘になる) */
  const applyLoaded = (v: GameView) => {
    g.setMoveSource({});
    g.setThinkTotal({ black: 0, white: 0 });
    g.setPlaying(false);
    g.setView(v);
    g.say('');
  };
  const loadFromFile = async () => {
    try {
      const loaded = await api.loadKifu();
      if (loaded) { applyLoaded(loaded); setPaste(false); }   // null = 選ばずに閉じた
    } catch (e) { g.say('' + e); }
  };
  /** 読み込んで、必要ならその手数まで進める (控えの明細から飛ぶときに使う)。 */
  const loadFromText = async (text: string, ply?: number) => {
    try {
      applyLoaded(await api.loadKifuText(text));
      setPaste(false);
      if (ply !== undefined) await g.jumpTo(ply);
    } catch (e) { g.say('' + e); }
  };

  /* GGS からの一言と、取り出した棋譜を受け取る。
   *
   * どちらもバックエンドが載せっぱなしにするので、**受け取ったら消す**。
   * 消さないと画面を開き直すたびに同じ報せが出るし、次に取り出した棋譜と
   * 見分けが付かない。 */
  const notice = ggs.snap?.notice ?? '';
  const fetched = ggs.snap?.fetched_ggf ?? null;
  useEffect(() => {
    if (!notice) return;
    void (async () => {
      g.say(notice);
      await ggsApi.ack().catch(() => {});
    })();
    // g は毎描画で作り直されるので依存に入れない (notice が変わったときだけ)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [notice]);
  useEffect(() => {
    if (!fetched) return;
    void (async () => {
      // 覆いが待っているならそちらへ差し込む。待っていないときだけ検討へ
      if (fetched.ggf) {
        setViewer((cur) => (cur && cur.pending === fetched.id ? { ...cur, kifu: fetched.ggf } : cur));
        // 覆いが待っていないときだけ検討へ。viewer は依存に入れない
        // (入れると覆いを開くたびにこの effect が走り直す)
        if (!viewer?.pending) {
          setNav('study');
          await loadFromText(fetched.ggf);
        }
      } else {
        setViewer(null);
        g.say(fetched.error || '棋譜を取り出せません');
      }
      await ggsApi.ack().catch(() => {});
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fetched]);

  const v = g.view;
  const moves = useMemo(
    () => (v ? movesOf(v, g.moveSource, graph.values) : []),
    [v, g.moveSource, graph.values]);
  const evals = g.autoHint ? evalsOf(g.hints) : undefined;
  // 敗着 = いちばん損した手。帯とグラフの両方が同じものを指すように 1 か所で出す
  const blunder = useMemo(() => {
    let best: { at: number; loss: number } | undefined;
    for (const m of moves) if (m.loss && (!best || m.loss > best.loss)) best = { at: m.n, loss: m.loss };
    return best;
  }, [moves]);

  /* キー操作。
   *
   * 文字を打っている最中 (GGS のチャット・コンソール・棋譜の貼り付け) と、
   * 覆いが出ている間は何もしない — 打った文字が画面を切り替えてしまう。
   * 覆いの中の操作は覆い自身が持つ (Esc など)。 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      if (settings || paste || ask) return;
      const cmd = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();
      if (cmd && key === 'b') { e.preventDefault(); setNav('book'); return; }
      if (cmd && key === 'n') { e.preventDefault(); if (!g.thinking) void g.newGame(); return; }
      // 棋譜の出し入れ。GGS と定石では扱う棋譜が無い
      if (cmd && key === 's' && !isGgs && !isBook) {
        e.preventDefault();
        void api.saveKifu(...ggfNames(g.side)).catch((err) => g.say('' + err));
        return;
      }
      if (cmd && key === 'o' && !isGgs && !isBook) { e.preventDefault(); setPaste(true); return; }
      if (cmd && key === 'z') {
        e.preventDefault();
        if (!g.thinking && v && v.move_count > 0) void g.undo();
        return;
      }
      // 手順を行き来するのは検討と定石。対局中に矢印で戻せると、打った手が
      // 消えたのか戻したのか分からなくなる
      if (isBook) {
        if (e.key === 'ArrowLeft') { e.preventDefault(); book.back(); }
        if (e.key === 'ArrowUp') { e.preventDefault(); book.reset(); }
        // 右は「いちばん値の高い手へ進む」。盤を見ながら本筋をなぞれる
        if (e.key === 'ArrowRight' && book.node?.moves.length) {
          e.preventDefault();
          book.push(book.node.moves[0].pos);
        }
        return;
      }
      if (!study || !v) return;
      // ⌘ で端まで、⇧ で 10 手、素で 1 手
      const step = e.shiftKey ? 10 : 1;
      if (e.key === 'ArrowLeft') { e.preventDefault(); void g.jumpTo(cmd ? 0 : Math.max(0, v.cursor - step)); }
      if (e.key === 'ArrowRight') {
        e.preventDefault();
        void g.jumpTo(cmd ? v.moves.length : Math.min(v.moves.length, v.cursor + step));
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [setNav, settings, paste, ask, isBook, isGgs, study, book, g, v]);

  const toasts: Toast[] = g.toasts.map(t => ({ id: String(t.id), tone: t.tone, text: t.text }));

  const over = v?.over ?? false;
  const result = v?.over
    ? (v.black === v.white ? '引き分け' : v.black > v.white ? '黒の勝ち' : '白の勝ち')
    : undefined;
  const anyThink = g.thinkTotal.black > 0 || g.thinkTotal.white > 0;
  const nodes = g.stat && g.stat.nodes > 0 ? g.stat.nodes : 0;
  const nps = nodes && g.stat && g.stat.secs > 0 ? nodes / g.stat.secs : 0;
  const lv = g.level === 'custom' ? 'カスタム' : LEVELS[g.level].name;
  // 「自分が下」は、KUROOBI が持っていない色 = 人が打つ色を下にする
  const sideColor = g.side === 'black' ? 'white' : g.side === 'white' ? 'black' : '';

  return (
    <AppFrame>
      <Toolbar
          dock={isGgs ? undefined : { open: dockOpen, onToggle: () => setDockOpen(o => !o) }}
          aux={isGgs || isBook ? undefined : <Toggle checked={g.autoHint} onChange={g.setAutoHint} label="評価値" />}>
          {isBook ? (
            <>
              <Button disabled={!book.line.length} onClick={book.back}>戻る</Button>
              <Button disabled={!book.line.length} onClick={book.reset}>最初へ</Button>
              {/* 定石で見つけた手順をそのまま検討へ。定石の先を自分で読ませる
                  にはこの向きの道が要る (行きだけあって帰りが無かった) */}
              <Button disabled={!book.line.length}
                      onClick={() => {
                        setNav('study');
                        void loadFromText(book.line.map(sqName).join(''));
                      }}>検討で開く</Button>
              <span style={{ marginLeft: 'var(--sp-3)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                {book.line.length ? book.line.length + ' 手目' : '初期局面'}
              </span>
            </>
          ) : isGgs ? (
            <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>
              {conn === 'online' ? <>接続中 <b style={{ color: 'var(--text)' }}>{ggs.snap?.login}</b></>
                : conn === 'offline' ? '未接続' : 'ログインしています…'}
            </span>
          ) : study ? (
            // 検討では KUROOBI は打たない。操作は手順を行き来することだけで、
            // 分析はグラフの見出し行が持つ (押す場所と結果の出る場所を離さない)
            <>
              <Button disabled={!v || v.cursor === 0} onClick={() => void g.jumpTo(0)}>最初へ</Button>
              <Button disabled={!v || v.cursor === 0} onClick={() => void g.jumpTo(v!.cursor - 1)}>戻る</Button>
              <Button disabled={!v || v.cursor >= v.moves.length}
                      onClick={() => void g.jumpTo(v!.cursor + 1)}>進む</Button>
              <Button disabled={!v || v.cursor >= v.moves.length}
                      onClick={() => void g.jumpTo(v!.moves.length)}>最後へ</Button>
              {/* いまの局面から定石を辿る。ここが無いと、検討で見ている手順を
                  初期局面から入れ直すしかない (打ち直しでしか行けない) */}
              <Button disabled={!g.hasBook || !v}
                      onClick={() => {
                        book.goto(v!.moves.slice(0, v!.cursor)
                          .filter((m): m is number => m != null).map(sqName).join(''));
                        setNav('book');
                      }}>定石で開く</Button>
            </>
          ) : (
            <>
              <Button variant={g.playing ? 'danger' : 'primary'}
                      disabled={!g.playing && (over || g.thinking)}
                      onClick={startGame}>
                {g.playing ? '対局停止' : '対局開始'}
              </Button>
              <Button disabled={g.thinking} onClick={() => void g.newGame()}>新規対局</Button>
              <Button disabled={g.thinking || !v || v.move_count === 0}
                      onClick={() => void g.undo()}>待った</Button>
              {/* 担当は狭い窓でも変えられないと困るので aux ではなく children 側。
                  aux は 940px で消える */}
              <span style={{ marginLeft: 'var(--sp-3)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>KUROOBI</span>
              <Segmented value={g.side} onChange={g.setSide} options={SIDES} />
            </>
          )}
      </Toolbar>

      <Body>
      <Nav items={NAV_LOCAL} ggsItems={ggsNav(conn, navBadges(ggs.snap, chatUnread))} conn={conn}
           active={nav} onSelect={setNav}
           footer={<>
             {cpu && <>
             {/* 使用率の上限はコア数 × 100%。溝はその割合で埋める */}
             <Meter icon="cpu" label="CPU" value={Math.round(cpu.cpu)} unit="%"
                    ratio={cpu.cpu / (cpu.cores * 100)} />
             {/* 単位の前の空白は flex の中で潰れるので、潰れない空白を使う */}
             <Meter icon="memory" label="メモリ" value={(cpu.mem / 1e9).toFixed(1)} unit={'\u00a0GB'}
                    ratio={cpu.mem_total > 0 ? cpu.mem / cpu.mem_total : 0} />
             <JobList jobs={jobsOf(cpu)} />
             </>}
             {/* 設定はいちばん下。行き先ではないので行の並びには入れない。
                 48px の列では文字を落として絵だけにする — 素の Button だと
                 幅が足りずに文字がはみ出す */}
             <button type="button" className="k-press" title="設定" aria-label="設定"
                     onClick={() => setSettings(true)}
                     style={{
                       display: 'flex', alignItems: 'center', justifyContent: 'center',
                       gap: 'var(--sp-2)', height: 'var(--h-field)', width: '100%',
                       border: '1px solid var(--border)', borderRadius: 'var(--r-2)',
                       background: 'var(--card)', color: 'var(--text)',
                       fontSize: 'var(--fs-6)', cursor: 'pointer', padding: 0,
                     }}>
               <Icon name="gear" size={15} />
               <span className="k-nav-label">設定</span>
             </button>
           </>} />

      <Main>
        {/* 盤の上の帯は「対局の操作」と、対局の前提を決める最小限だけ。
            思考中の数字は下の帯へ */}

        {isGgs ? <GgsScreen nav={nav} snap={ggs.snap} onNav={setNav} prefs={prefs}
                       onKifu={(title, kifu, archive) => {
                         setViewer({ title, kifu, pending: kifu ? undefined : archive });
                         if (!kifu && archive) void ggsApi.look(archive);
                       }} />
         : isBook ? (
          <BookPane b={book} coords={prefs.coords} grain={prefs.grain}
                    flip={flipped(prefs.facing, '')} onSettings={() => setSettings(true)} />
        ) : (
        <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', padding: '0 var(--sp-4)' }}>
          <PlayerRow color="b" name="黒" discs={v?.black ?? 2}
                     active={!!v && !v.over && v.player === 'black'}
                     clock={!study && anyThink ? fmtSecs(g.thinkTotal.black) : undefined} />
          <div style={{ flex: 1, minHeight: 0, display: 'grid', placeItems: 'center', padding: 'var(--sp-2) 0' }}>
            <div style={{ height: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
              {v && <Board cells={cellsOf(v)} legal={v.legal} last={v.last} evals={evals}
                           coords={prefs.coords} grain={prefs.grain}
                           // 検討では「自分の色」が無いので、auto は黒が下のまま
                           flip={flipped(prefs.facing, study ? '' : sideColor)}
                           disabled={g.thinking}
                           onPlay={(sq) => void g.play(sq)} />}
            </div>
          </div>
          <PlayerRow color="w" name="白" discs={v?.white ?? 2}
                     active={!!v && !v.over && v.player === 'white'}
                     meta={result}
                     clock={!study && anyThink ? fmtSecs(g.thinkTotal.white) : undefined} />
        </div>
        )}

        {/* GGS の対局中に開く下の板。チャットとコンソールが 1 枚を共有する */}
        {isGgs && panel && ggs.snap && (
          <BottomPanel
            tabs={[{ id: 'chat', label: 'チャット', unread: panel === 'chat' ? 0 : chatUnread },
                   { id: 'console', label: 'コンソール' }]}
            active={panel} onTab={(id) => showPanel(id as 'chat' | 'console')}
            onClose={() => showPanel('')}>
            {panel === 'chat' ? <GgsChat snap={ggs.snap} /> : <GgsConsole snap={ggs.snap} />}
          </BottomPanel>
        )}

        {/* 手数を辿る帯。分析していなくても辿れるので、グラフより先に置く */}
        {study && !isGgs && !isBook && v && (
          <MoveScrub plies={v.moves.length} cursor={v.cursor} blunder={blunder}
                     onSeek={(n) => void g.jumpTo(n)} />
        )}

        {/* 箱に入れず、盤の下の帯として全幅に置く。検討だけ */}
        {study && !isGgs && !isBook && (
          <EvalGraph points={graph.values ?? []} plies={v?.moves.length} cursor={v?.cursor}
                     blunder={blunder} busy={graph.busy} onJump={(n) => void g.jumpTo(n)}
                     extra={<>
                       {/* 進み具合は見出し行に出す。帯の高さは変えない —
                           出るたびに枠が伸びると下の段が全部カタカタ動く */}
                       {/* 数字の隣に細い帯を並べる (規則 69)。数字だけだと
                           残りの見当が付かない */}
                       {graph.prog && (
                         <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
                           分析中 <b style={{ color: 'var(--text)' }}>{graph.prog.done}</b>/{graph.prog.total}
                           <span style={{ width: 72 }}>
                             <Progress value={graph.prog.total > 0 ? graph.prog.done / graph.prog.total : 0} />
                           </span>
                         </span>
                       )}
                       <Button size="chip" variant={graph.busy ? 'danger' : 'secondary'}
                               onClick={() => (graph.busy ? graph.stop() : void graph.update())}>
                         {graph.busy ? '分析停止' : '分析'}
                       </Button>
                     </>} />
        )}

      </Main>

      {/* GGS はドックを持たない (一覧が本体の左に付く) */}
      {/* 定石が無いときは空のドックも出さない (盤側が報せを出している) */}
      {isBook && book.node?.size !== 0 && (
        <Dock tabs={['定石']} active="定石" open={dockOpen}>
          <BookDock b={book} onStudy={(kifu) => { setNav('study'); void loadFromText(kifu); }} />
        </Dock>
      )}

      {/* 棋譜のときは丸ごとスクロールさせない — 表が列の見出しを固定し、
          行だけを流す作りになっている (操作も上に残す) */}
      {!isGgs && !isBook && (
      <Dock tabs={['棋譜', '強さ', '学習']} active={tab} onTab={setTab} open={dockOpen}
            scroll={tab !== '棋譜'}>
        {tab === '棋譜' && (
          <>
            {/* 分析 (評価値グラフ) は検討画面のもの。対局には置かない */}
            <div style={{ display: 'flex', gap: 'var(--sp-2)', padding: 'var(--sp-2) var(--sp-3)' }}>
              {/* .ggf で保存すると、どちらがどの色か・結果・開始局面まで入る */}
              <Button size="chip"
                      onClick={() => void api.saveKifu(...ggfNames(g.side)).catch((e) => g.say('' + e))}>
                保存
              </Button>
              <Button size="chip" onClick={() => setPaste(true)}>読込</Button>
            </div>
            <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
              <KifuTable moves={moves} current={v?.cursor}
                         onSelect={(n) => void g.jumpTo(n)} />
            </div>
          </>
        )}
        {tab === '強さ' && (
          // 検討では同じ 3 つが解析の深さになる。値も操作も同じなので節の名前だけ変える
          <Section title={study ? '解析の設定' : '強さ'}>
            {/* 選び方は GGS の設定と共通 (Strength)。同じ 3 つを決めるのに
                操作が違うと、片方で覚えたことがもう片方で通じない */}
            <Strength value={g.levels} onChange={(v) => {
              const i = LEVELS.findIndex((l) => l.depth === v.depth && l.solve === v.solve && l.band === v.band);
              if (i >= 0) { g.setLevel(i); return; }
              g.setCustom(v);
              g.setLevel('custom');
            }} />
            <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>定石</span>
              <Segmented value={g.useBook ? 'on' : 'off'} disabled={!g.hasBook}
                         onChange={(x) => g.setUseBook(x === 'on')}
                         options={[{ value: 'on', label: '使う' }, { value: 'off', label: '使わない' }]} />
            </div>
            {!g.hasBook && (
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                — ファイルがありません (設定から指定できます)
              </span>
            )}
          </Section>
        )}
        {tab === '学習' && (
          <>
            <Section title="定石への書き戻し">
              <Toggle checked={g.learnOn} onChange={g.setLearnOn} label="終局した対局を取り込む" />
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8 }}>
                勝敗にかかわらず取り込み、終局の石差を根まで書き戻します。同じ負け方をなぞらなくなります。
              </span>
            </Section>
            {/* 取り込みは裏で静かに進むので、何が入ったかを見る場所が要る。
                対局 → 敗着 → 書き換えた手 (旧→新) まで一本で辿れる */}
            <LearnLog items={learnLog}
              onUndo={(e) => void (async () => {
                if (!await confirm('この対局で書き換えた定石を元に戻します。よろしいですか。')) return;
                try {
                  await api.learnUndo(e.at, e.kifu);
                  // 済んだことは報せない (デザイン規則 34)。行が消えるのが
                  // 結果そのもので、トーストは失敗と進まない理由だけに使う
                  learnLogReload();
                } catch (err) { g.say('' + err); }
              })()}
              onOpen={(e, ply) => {
              setNav('study');
              // 抽選開局は開始局面を頭に付けないと別の対局になる
              void loadFromText(e.start ? e.start + '\n' + e.kifu : e.kifu, ply);
            }} />
          </>
        )}
      </Dock>
      )}

      </Body>

      {/* 短くて桁の決まっているものだけを置く。長さの読めない報せはトーストへ */}
        <StatusBar
          left={<>
            {g.thinking && <StatusStat label="思考" value={g.thinkSecs.toFixed(1)} unit="s" />}
            {nodes > 0 && <StatusStat label="nodes" value={fmtNodes(nodes)} />}
            {nps > 0 && <StatusStat label="nps" value={(nps / 1e6).toFixed(1)} unit="Mnps" />}
          </>}
          right={isGgs
            ? <>
              {/* 対局中だけ出す。観戦や一覧を見ているときに出すと、
                  左の並びと同じものが 2 か所にあるだけになる */}
              {ggsMatch && <>
                <StatusChip label="チャット" unread={panel === 'chat' ? 0 : chatUnread}
                            active={panel === 'chat'}
                            onClick={() => showPanel('chat')} />
                <StatusChip label="コンソール" active={panel === 'console'}
                            onClick={() => showPanel('console')} />
              </>}
              <StatusStat label="GGS" value={conn === 'online' ? '接続中' : conn === 'offline' ? '未接続' : '接続しています…'} />
            </>
            : isBook
            ? <StatusStat label="定石" value={book.node ? book.node.size.toLocaleString() + ' 局面' : '—'} />
            : <>
              <StatusStat label="定石" value={g.hasBook ? (g.useBook ? '有効' : '使わない') : 'なし'} />
              <StatusStat label="KUROOBI" value={lv} />
            </>} />

      {settings && (
        <Settings prefs={prefs} setPref={setPref}
                  learnOn={g.learnOn} onLearn={g.setLearnOn}
                  onChanged={() => void api.hasBook().then(g.setHasBook).catch(() => {})}
                  onClose={() => setSettings(false)} />
      )}

      {ask && (
        <Confirm title="確認" body={ask.msg} ok="続ける"
                 onCancel={() => { ask.done(false); setAsk(null); }}
                 onOk={() => { ask.done(true); setAsk(null); }} />
      )}

      {viewer && (
        <KifuViewer title={viewer.title} kifu={viewer.kifu}
                    onClose={() => setViewer(null)}
                    onStudy={(text) => { setNav('study'); void loadFromText(text); }} />
      )}

      {paste && (
        <PasteKifu onCancel={() => setPaste(false)}
                   onFile={() => void loadFromFile()}
                   onLoad={(t) => void loadFromText(t)} />
      )}

      <Toasts items={toasts} onDismiss={(id) => g.dismiss(+id)} />
    </AppFrame>
  );
}

/** GGF に載せる対局者名。KUROOBI が持っている色にその名を置く。
 *  GGF は他のソフトが読むので ASCII に留める。 */
function ggfNames(side: 'black' | 'white' | 'both' | 'off'): [string, string] {
  if (side === 'both') return ['KUROOBI', 'KUROOBI'];
  if (side === 'black') return ['KUROOBI', 'Player'];
  if (side === 'white') return ['Player', 'KUROOBI'];
  return ['Player', 'Player'];
}

/** 桁が伸びても幅が暴れないように短く畳む。 */
const fmtNodes = (n: number): string =>
  n >= 1e9 ? (n / 1e9).toFixed(1) + 'G' : n >= 1e6 ? (n / 1e6).toFixed(1) + 'M'
    : n >= 1e3 ? (n / 1e3).toFixed(0) + 'k' : String(n);

/** 走っている仕事。左メニューの下に常時出す。
 *  ローカル探索と GGS 対局は別スレッドプールなので、両方を別の行として出す。 */
function jobsOf(cpu: ActivityView) {
  const jobs: { label: string; threads?: number; yielded?: boolean }[] = [];
  if (cpu.local) jobs.push({ label: cpu.local, threads: cpu.local_threads });
  if (cpu.ggs_match) jobs.push({ label: 'GGS 対局', threads: cpu.ggs_thinking ? cpu.ggs_threads : undefined });
  if (cpu.learn) {
    jobs.push({ label: `学習 ${cpu.learn[0]}/${cpu.learn[1]}`, yielded: cpu.learn_paused });
  }
  return jobs;
}
