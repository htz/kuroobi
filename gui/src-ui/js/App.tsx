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
import { EvalGraph, MoveScrub, ScoreRow, StoneDot } from './components/data';
import { GgsStatus, JobList, Meter, Nav, NAV_LOCAL, StatusChip, ggsNav, Toasts, type NavId, type Toast } from './components/ggs';
import { Button, Progress, Segmented, Toggle } from './components/primitives';
import { Icon } from './components/Icons';
import { BookDock, BookPane, BookTree, useBookBrowse } from './BookScreen';
import { LearnLog } from './LearnLog';
import { KifuViewer } from './KifuViewer';
import { LEVELS } from './state';

/* 対局と検討の画面。
 *
 * エンジンとのやりとりは engine.ts が持つ (旧 App と同じものを使う)。
 * ここがやるのは「状態を画面の形にして並べる」だけ。
 */

/* 担当の選択肢には石を添える (規則 59)。名前と石が並べば「KUROOBI が白を
 * 持つ」と読めるので、それ以上の語が要らない。「なし」だけは人が両方を打つ
 * ので石を出さない — 出すと「両方」との違いが石では付かなくなる。 */
const SIDES = [
  { value: 'black' as const, label: <><StoneDot color="b" />黒</> },
  { value: 'white' as const, label: <><StoneDot color="w" />白</> },
  {
    // 2 つの石は重ねない。9px で重ねると輪郭が潰れて 1 つの黒い塊に見え、
    // ライトでは白石の縁が地に紛れてなおさら分からない
    value: 'both' as const,
    label: <><span style={{ display: 'flex', gap: 2 }}>
      <StoneDot color="b" /><StoneDot color="w" />
    </span>両方</>,
  },
  { value: 'off' as const, label: 'なし' },
];


/** いま実際に行ける行き先へ読み替える。左メニューから消えた行に居座らせない。 */
function reachable(raw: NavId, conn: ReturnType<typeof connOf>): NavId {
  if (conn === 'online') return raw === 'ggs-login' ? 'ggs-play' : raw;
  return raw.startsWith('ggs-') && raw !== 'ggs-login' ? 'ggs-login' : raw;
}

export function App() {
  const { prefs, set: setPref } = usePrefs();
  /* 持ち時間は設定 (`prefs`) が持つ。**対局の状態ではなく好み** —
     アプリを閉じても残ってほしいし、対局中に変わると時計が飛ぶ */
  const g = useGame(prefs.clockSecs);
  const ggs = useGgs();
  const [navRaw, setNavRaw] = useState<NavId>('play');
  const conn = connOf(ggs.snap?.conn);
  /* 繋がり方が変わると左メニューの GGS の行が総入れ替わりになる (規則 10 —
   * 未接続はログイン 1 行、接続後は 7 行)。**行が消えても居場所は残る**ので、
   * ログインが通っているのにログイン画面が出たままだった (上の帯と下の帯は
   * 「接続中」を出しているのに中央だけ古い)。いる行が消えたら、その時点で
   * 行ける先へ読み替える。状態を書き換えず導くのは、繋がり方が先に変わって
   * から描き直す順を保つため (効果の中で書き換えると 1 枚古い絵が挟まる)。 */
  const nav = reachable(navRaw, conn);
  const study = nav === 'study';
  const isBook = nav === 'book';
  const isGgs = nav.startsWith('ggs');
  const [tab, setTab] = useState('棋譜');
  /* 定石の画面は 2 枚 — 木と盤の「定石」、書き戻しの明細の「学習ログ」。
     行き先は 1 つのままにする (左メニューを増やさない)。独立ウィンドウは
     取りやめになったので、ここが §7 と §8 の置き場所になる */
  const [bookTab, setBookTab] = useState('定石');
  const [dockOpen, setDockOpen] = useState(false);
  /* 評価値グラフの帯。620px 以下でだけ畳む (base.css)。広い窓では
     この値は使われない — 畳む段が持つので、閉じたまま窓を広げても出る */
  const [graphOpen, setGraphOpen] = useState(false);
  /* 見る側 (設計 §2 の「黒視点 / 白視点」)。**盤の回転ではなく評価値の符号**で、
     棋譜表・グラフ・盤のすべてに同じ向きで効く (設計の設定「表示」の注記)。
     検討だけが切り替えられる — 対局中に符号が反ると、指しながら読む数字の
     意味が変わって危ない */
  const [pov, setPov] = useState<'b' | 'w'>('b');

  // 設定は別の窓なので、そこでファイルを差し替えてもこちらは気付けない。
  // 報せを聞いて定石の有無だけ取り直す (盤の「定石」表示が変わる)
  const setHasBook = g.setHasBook;
  useEffect(() => {
    const off = onApp('resources-changed', () => {
      void api.hasBook().then(setHasBook).catch(() => { /* エンジン未初期化 */ });
    });
    return () => { void off.then((f) => f()); };
  }, [setHasBook]);

  useEngineSettings(g);
  useHints(g);
  useEngineTurn(g);
  const cpu = useActivity();
  // engine.ts は画面を持たないので、確認はこちらが出して答えだけ返す
  const [ask, setAsk] = useState<(AskArgs & { done: (ok: boolean) => void }) | null>(null);
  const confirm = useCallback(
    (a: AskArgs) => new Promise<boolean>((done) => setAsk({ ...a, done })),
    [setAsk]);
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
    if (navRaw === 'ggs-chat' && id !== 'ggs-chat') setChatSeen(chatTotal);
    setNavRaw(id);
    if (id === 'play' || id === 'study') setMode(id === 'study' ? 'study' : 'vs');
  }, [setMode, navRaw, chatTotal]);

  // ドックの学習タブでも登録局面の数を出すので、そこでも節を取る
  const book = useBookBrowse(isBook || tab === '学習');
  const { items: learnLog, reload: learnLogReload } = useLearnLog(
    isBook && bookTab === '学習ログ', !!cpu?.learn);

  /* 棋譜ビューア (規則 71)。`pending` はアーカイブ番号 — 手元に棋譜が
   * 無い対局は覆いを先に開き、届いたら中身を差し込む。 */
  const [viewer, setViewer] = useState<
    { title: string; kifu: string; pending?: string; archive?: string; parts?: string[] } | null
  >(null);

  const [paste, setPaste] = useState(false);
  /* 設定は覆い。**窓にしていたのをやめた** — 窓である値打ちは「表示」タブ
     だけが持っていたのに、1240 幅の主画面では盤に重なるのを避けられず
     (直したあとでも 12%)、そのために localStorage 越しの同期・窓の札・
     置き場所の計算まで抱えていた。割に合わない (要 push — 設計 §5 は
     「独立ウィンドウ (⌘,)」と描いている) */
  const [settings, setSettings] = useState(false);
  /** 設定を開いたときの最初のタブ (撮るためだけの入口)。 */
  const [settingsTab, setSettingsTab] = useState<'engine' | 'view' | 'ggs'>('engine');

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
      const [who, lv, extraRaw] = v.split(':');
      /* `:nobook` は末尾に付く目印で、ドックの見出しではない。位置で
         受けていたので `both:40:nobook` と書くと見出しが "nobook" になり、
         **どの枚も出ない = 画面が真っ白**になっていた (実際に踏んだ)。
         見出しの位置に来ても目印として扱う */
      // 末尾の目印 (`nobook` / `clock<秒>`) は見出しではない
      const extra =
        extraRaw === 'nobook' || /^clock\d+$/.test(extraRaw ?? '') ? undefined : extraRaw;
      /* `:nobook` は定石を切ってから始める目印。**どの入口より先に効かせる** —
         `study` などの分岐が `return` するので、下に置くと届く入口と届かない
         入口ができる (実際 `study:graph:nobook` で切れていなかった)。
         対局では**序盤が定石から返ると探索を呼ばない**ので、学習が譲る条件
         (`Activity.local`) が立たず「譲り中」の行を撮れない。分析では
         定石の点が出るかどうかの出し分けを確かめるのに要る */
      if (v.endsWith(':nobook')) g.setUseBook(false);
      /* `both:6:clock60` — 末尾に付ける目印 (`nobook` と同じ形)。持ち時間を
         入れてから始める (撮るためだけの入口)。設定を押さないと時計が
         動かず、実機で確かめられなかった。
         **`api.setClock` も直に呼ぶ** — この経路は「新規対局」を通らないので、
         `prefs` を変えるだけでは時計が初期化されない */
      const mc = v.match(/:clock(\d+)$/);
      if (mc) { setPref('clockSecs', +mc[1]); void api.setClock(+mc[1]); }
      if (who === 'settings') {
        // `settings:ggs` のようにタブを指定できる (撮るためだけ)
        if (lv === 'ggs' || lv === 'view' || lv === 'engine') setSettingsTab(lv);
        setSettings(true);
        return;
      }
      /* 覆いを出す入口 (撮るためだけ)。**覆いはクリックでしか出せず、
         寸法をずっと実測できていなかった** — 確認 / 棋譜の読み込み /
         棋譜ビューア。`overlay:confirm` のように指定する */
      if (who === 'overlay') {
        if (lv === 'paste') { setPaste(true); return; }
        if (lv === 'confirm') {
          void confirm({ title: '27 手目まで戻します',
                         body: '自分の手と KUROOBI の手を 1 手ずつ戻します。戻した先から指し直せます。',
                         ok: '戻す' });
          return;
        }
        if (lv === 'toast') {
          // 2 つ積んだところを撮る (設計 §9 は 2 枚を溝 10 で重ねている)
          g.say('GGS 対局中は分析を控えます', 'gold');
          setTimeout(() => g.say('棋譜がありません'), 150);
          return;
        }
        if (lv === 'viewer') {
          setViewer({ title: 'saio との対局',
                      kifu: 'e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2' });
          return;
        }
        return;
      }
      /* "yield" — **学習が譲っているところを撮るための入口**。
         `learn_paused` は `Activity.local` (思考 / 分析) が立っている間だけ
         true になるが、対局の序盤は定石から手が返るので探索を呼ばず、
         中盤に入る頃には 1 局ぶんの取り込み (60 局面) が終わっている。
         **同じプロセスの中で「対局が終わる → 取り込みが始まる →
         もう一度探索を始める」を作らないと重ならない。**
         取り込みが始まるのを待ってから検討の分析を重ねる。 */
      if (who === 'yield') {
        setTab('学習');
        g.setSide('both');
        g.setLevel(0);   // いちばん速い。早く終局させて取り込みを始めさせる
        g.setPlaying(true);
        void (async () => {
          for (let i = 0; i < 240; i++) {
            await new Promise((r) => setTimeout(r, 500));
            const a = await api.activity().catch(() => null);
            if (a?.learn) break;
          }
          /* **もう一局、こんどは深く読ませる。**検討の分析では重ならない —
             1 局面ずつ短く印を立てるだけで、値が控えてあると一瞬で終わる。
             譲りが続くのは**1 手に秒を使う探索**のときだけ */
          await g.newGame();
          // **定石を切る。**序盤が定石から返ると探索を呼ばないので、
          // 深さを上げても 1 手目から譲りが起きない
          g.setUseBook(false);
          g.setLevel(12);
          g.setPlaying(true);
        })();
        return;
      }
      // "tab:学習" のようにドックの見出しを指定する (撮るためだけの入口)。
      // "tab:強さ:custom" なら強さをカスタムにして 3 枠を開く
      if (who === 'tab') {
        if (lv) setTab(lv);
        if (v.endsWith(':custom')) { g.setCustom({ depth: 20, solve: 24, band: 4 }); g.setLevel('custom'); }
        return;
      }
      // "book:f5d6" のように手順を渡すと、その節まで辿った状態で開く。
      // "book:log" は 2 枚目 (学習ログ) を開く — 撮るためだけの入口
      if (who === 'book') {
        setNavRaw('book');
        if (lv === 'log') setBookTab('学習ログ');
        else if (lv) bookLine.current?.(lv);
        return;
      }
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
        // "study:graph:学習" のようにドックの見出しも指定できる。分析は
        // `Activity.local` を立てるので、**学習が譲っている見た目**はここで撮れる
        if (extra) setTab(extra);
        return;
      }
      if (who === 'both') g.setSide('both');
      if (lv !== undefined && Number.isFinite(+lv)) g.setLevel(+lv);
      // "both:1:学習" のようにドックの見出しも指定できる。取り込みの状況は
      // 対局が終わって学習が走っている間しか出ないので、その枠を撮る入口
      if (extra) setTab(extra);
      g.setPlaying(true);
    }).catch((e) => jsLog('autoplay: ' + e));
    // g は毎描画で作り直されるので依存に入れない (started で 1 度だけに絞っている)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => { autoGraph.current = () => void graph.update(); }, [graph]);
  useEffect(() => { bookLine.current = book.goto; }, [book.goto]);

  /* 画面確認用 (KUROOBI_GGS_AUTOVIEW=players のように指定する)。
   * GGS の画面は開くまで描かれないので、撮るには行き先を指定する経路が要る。 */
  /* 環境変数の上書き。起動時に 1 度だけ聞く (途中で変わらない) */
  const [envOverrides, setEnvOverrides] = useState<[string, string][]>([]);
  useEffect(() => { void api.envOverrides().then(setEnvOverrides).catch(() => {}); }, []);

  useEffect(() => {
    void ggsApi.autoview().then((v) => {
      // "users:card" のように画面の中の状態まで指定できる。行き先は前半だけ
      const to = v.split(':')[0];
      if (to) setNavRaw(('ggs-' + to) as NavId);
    }).catch(() => {});
  }, []);

  /** 読み込んだら手順の記録も消す (前の対局の評価が残ると嘘になる) */
  const applyLoaded = (v: GameView) => {
    g.setMoveSource({});
    g.setThinkTotal({ black: 0, white: 0 });
    g.setPlaying(false);
    g.setView(v);
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
        // 同期対局は 2 面入っている。覆いで面を選べるように全部渡す
        setViewer((cur) => (cur && cur.pending === fetched.id
          ? { ...cur, kifu: fetched.ggf, parts: fetched.parts }
          : cur));
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
  // 検討でだけ反す。対局・定石・GGS は黒視点のまま
  const sign: 1 | -1 = study && pov === 'w' ? -1 : 1;
  const moves = useMemo(
    () => (v ? movesOf(v, g.moveSource, graph.values, sign) : []),
    [v, g.moveSource, graph.values, sign]);
  const evals = g.autoHint ? evalsOf(g.hints, v?.player !== 'white', sign) : undefined;
  /* グラフに渡す点も見る側で反す。**名札だけ入れ替えて値を残すと嘘になる** —
     白視点にしたのに黒有利の点が「白有利」側へ伸びていた (実機で見つけた)。 */
  const povPoints = useMemo(
    () => (graph.values ?? []).map((p) => (p && sign === -1 ? { ...p, value: -p.value } : p)),
    [graph.values, sign]);
  // 敗着 = いちばん損した手。帯とグラフの両方が同じものを指すように 1 か所で出す
  const blunder = useMemo(() => {
    let best: { at: number; loss: number } | undefined;
    for (const m of moves) if (m.loss && (!best || m.loss > best.loss)) best = { at: m.n, loss: m.loss };
    return best;
  }, [moves]);

  /* 検討では**いま見ている手の評価**を石数の行に添える (設計 §2)。
     グラフは形の流れを見るもので、点を目で追って値を読み取るのは無理がある。
     手を送るたびに数字が 1 つ出ていれば、辿りながら読める。
     `moves` が既に 手・色・評価・損失 を持っているので、そこから引くだけ */
  const cur = v && v.cursor > 0 ? moves[v.cursor - 1] : undefined;
  const curMoveMeta = cur && cur.score !== undefined ? (
    <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
      この手の評価
      <b style={{
        fontSize: 'var(--fs-3)', fontWeight: 600, color: 'var(--text)',
        fontVariantNumeric: 'tabular-nums',
      }}>{cur.score > 0 ? '+' : ''}{cur.score.toFixed(1)}</b>
      {/* 損は色で言う。数字だけ並べると「良い手だが値が低い」局面と読み違える */}
      {!!cur.loss && (
        <span style={{ color: 'var(--bad)', fontVariantNumeric: 'tabular-nums' }}>
          ▼{cur.loss.toFixed(1)}
        </span>
      )}
      <span>{cur.pass ? 'パス' : cur.move} · {cur.color === 'b' ? '黒' : '白'}</span>
      {cur.src && <span style={{ color: cur.src.startsWith('定石') ? 'var(--gold)' : 'var(--sub)' }}>{cur.src}</span>}
    </span>
  ) : undefined;

  /* キー操作。
   *
   * 文字を打っている最中 (GGS のチャット・コンソール・棋譜の貼り付け) と、
   * 覆いが出ている間は何もしない — 打った文字が画面を切り替えてしまう。
   * 覆いの中の操作は覆い自身が持つ (Esc など)。 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      /* **覆いが 1 枚でも出ていたら何もしない。**棋譜の貼り付けと確認しか
         見ていなかったので、**棋譜ビューアや設定を開いている間に ← → を
         押すと、後ろの盤が動いていた**。覆いを足すたびにここへ書き足す
         のではなく、覆いを 1 つにまとめて数える */
      if (paste || ask || viewer || settings) return;
      const cmd = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();
      /* 設定は覆い。**絵が §5 で「設定 — 覆い（⌘,）」と明記している**のに
         鍵が無く、左メニュー下端の釦からしか開けなかった。macOS の慣習でも
         あるので、押す前に試す人がいる */
      if (cmd && key === ',') { e.preventDefault(); setSettings(true); return; }
      if (cmd && key === 'b') { e.preventDefault(); setNav('book'); return; }
      if (cmd && key === 'n') { e.preventDefault(); if (!g.thinking) void g.newGame(); return; }
      // 棋譜の出し入れ。GGS と定石では扱う棋譜が無い
      // **釦と同じ条件で断る。**釦は 1 手も無いと沈むのに、鍵だけ通ると
      // 「棋譜が空です」が出て、押せない釦があることと辻褄が合わない
      if (cmd && key === 's' && !isGgs && !isBook) {
        e.preventDefault();
        if (v && v.moves.length > 0) {
          void api.saveKifu(...ggfNames(g.side)).catch((err) => g.say('' + err));
        }
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
  }, [setNav, paste, ask, viewer, settings, isBook, isGgs, study, book, g, v]);

  const toasts: Toast[] = g.toasts.map(t => ({ id: String(t.id), tone: t.tone, text: t.text }));

  const over = v?.over ?? false;
  const result = v?.over
    ? (v.black === v.white ? '引き分け' : v.black > v.white ? '黒の勝ち' : '白の勝ち')
    : undefined;
  const anyThink = g.thinkTotal.black > 0 || g.thinkTotal.white > 0;
  /** 石数の行に出す時計。持ち時間があれば残り、無ければ思考の累計。 */
  const clockLabel = (c: 'b' | 'w') => {
    if (g.clockSecs) {
      const v = c === 'b' ? g.clock?.black : g.clock?.white;
      return v === undefined ? undefined : fmtSecs(v);
    }
    return !study && anyThink ? fmtSecs(c === 'b' ? g.thinkTotal.black : g.thinkTotal.white) : undefined;
  };


  const nodes = g.stat && g.stat.nodes > 0 ? g.stat.nodes : 0;
  const nps = nodes && g.stat && g.stat.secs > 0 ? nodes / g.stat.secs : 0;
  const lv = g.level === 'custom' ? 'カスタム' : LEVELS[g.level].name;
  /* 窓の帯に出す「いま何を見ているか」。押せるものではなく受け身の文字
   * (規則 75)。行き先の名前は左の並びと同じものを使う — 2 か所で書くと割れる */
  const screenTitle =
    [...NAV_LOCAL, ...ggsNav(conn)].find((i) => i.id === nav)?.label ?? 'KUROOBI';
  const screenSub = isGgs
    ? (conn === 'online' ? ggs.snap?.login : undefined)
    : isBook ? undefined
    : `${lv} · ${g.side === 'both' ? '両方' : g.side === 'off' ? '担当なし'
        : g.side === 'black' ? '黒' : '白'}`;

  // 「自分が下」は、KUROOBI が持っていない色 = 人が打つ色を下にする
  const sideColor = g.side === 'black' ? 'white' : g.side === 'white' ? 'black' : '';

  return (
    <AppFrame>
      {/* 窓の層。押せるものは 1 つも置かない (規則 75) */}
      {/* **環境変数で起動した印は窓の帯の右端。** 付けると素の起動と違う
          ふるまいをする (レート戦が禁じられる / 繋ぐ先が作り物 / 控えの
          置き場所が違う など) ので、**それを知らずに触ると出ている絵が
          本物なのか確認用なのか区別が付かない**。
          左メニューの下端に置いていたが 2 つ間違っていた — そこは資源
          メーターの場所 (規則 9) で、**`--gold` は定石の色** (規則 19) なので
          金の意味が 2 つになる。窓の帯なら**どの画面でも同じ場所**で、
          畳む段の影響も受けない (全幅なので) */}
      <WindowBar title={screenTitle} sub={screenSub} right={<EnvTags items={envOverrides} />} />

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
                 **絵は歯車ではない** — 歯車は「GGS の設定」が使っているので、
                 同じ絵を別の意味で 2 か所に出さない (規則 49)。
                 48px の列では文字を落として絵だけの正方形にする */}
             <button type="button" className="k-press k-nav-settings" title="設定 (⌘,)" aria-label="設定"
                     onClick={() => setSettings(true)}
                     style={{
                       alignItems: 'center', justifyContent: 'center',
                       gap: 'var(--sp-2)', height: 'var(--h-field)',
                       border: '1px solid var(--border)', borderRadius: 'var(--r-2)',
                       background: 'var(--card)', color: 'var(--text)',
                       fontSize: 'var(--fs-6)', cursor: 'pointer', padding: 0,
                     }}>
               <Icon name="prefs" size={15} />
               <span className="k-nav-label">設定</span>
             </button>
           </>} />

      <Main inset={dockOpen && !isGgs}>
      <Toolbar
          dock={isGgs ? undefined : { open: dockOpen, onToggle: () => setDockOpen(o => !o) }}
          graph={study && !isGgs && !isBook
            ? { open: graphOpen, onToggle: () => setGraphOpen(o => !o) } : undefined}
          /* 定石の画面では取り込みの進み具合を右へ出す (絵 §8)。
             **走っているのは分かるが何が変わったか分からない**を無くすのが
             この画面の狙いなので、いま何局面目かが見えないと始まらない。
             走っていないときは行ごと出さない (規則 11) */
          aux={isBook
            ? (cpu?.learn
              ? <Busy>取り込み中<span style={{ color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                    局面 {cpu.learn[0].toLocaleString()} / {cpu.learn[1].toLocaleString()}
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
              {/* 2 枚の切り替えは children に置く — aux は 940px で消えるので、
                  狭い窓で学習ログへ行けなくなる (規則 8・58) */}
              <Segmented value={bookTab} onChange={setBookTab}
                         options={[{ value: '定石', label: '定石' },
                                   { value: '学習ログ', label: '学習ログ' }]} />
              {bookTab === '定石' && <>
                <Divider />
                <Button disabled={!book.line.length} onClick={book.back}>戻る</Button>
                <Button disabled={!book.line.length} onClick={book.reset}>最初へ</Button>
                <span style={{ marginLeft: 'var(--sp-3)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                  {book.line.length ? book.line.length + ' 手目' : '初期局面'}
                </span>
              </>}
            </>
          ) : isGgs ? (
            <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>
              {conn === 'online' ? <>接続中 <b style={{ color: 'var(--text)' }}>{ggs.snap?.login}</b></>
                : conn === 'offline' ? '未接続' : 'ログインしています…'}
            </span>
          ) : study ? (
            // 検討では KUROOBI は打たない。**送りの釦は手数の帯へ移した**
            // (設計 §2)。辿る道具を 1 か所にまとめる — 帯で掴んで滑らせるのと
            // 1 手ずつ送るのは同じ操作で、離して置くと ▶ を押した結果が
            // 別の場所に出る。分析はグラフの見出し行が持つ (同じ理由)。
            // 絵はここに 分析 / 棋譜を読み込む / 黒視点・白視点 を置いている
            // が、どれも今は別の場所にある (台帳に項目として積んだ)
            <>
              <Button variant="primary" disabled={graph.busy || !v?.moves.length}
                      onClick={() => void graph.update()}>分析</Button>
              <Button title="⌘O" onClick={() => setPaste(true)}>棋譜を読み込む</Button>
              <Divider />
              <Segmented value={pov} onChange={setPov}
                         options={[{ value: 'b', label: '黒視点' },
                                   { value: 'w', label: '白視点' }]} />
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                {v && v.moves.length ? `${v.cursor} / ${v.moves.length} 手` : '棋譜がありません'}
              </span>
            </>
          ) : (
            <>
              <Button variant={g.playing ? 'danger' : 'primary'}
                      disabled={!g.playing && (over || g.thinking)}
                      onClick={startGame}>
                {g.playing ? '対局停止' : '対局開始'}
              </Button>
              {/* **鍵は釦に書いておく。**押せることは見れば分かるが、
                  鍵があることは押しても分からない */}
              <Button title="⌘N" disabled={g.thinking} onClick={() => void g.newGame()}>新規対局</Button>
              <Button title="⌘Z" disabled={g.thinking || !v || v.move_count === 0}
                      onClick={() => void g.undo()}>待った</Button>
              {/* 押す操作と、対局の前提を決めるものは別の話なので縦罫で切る。
                  切らないと「新規対局」と「黒」が同じ並びに見える */}
              <Divider />
              {/* 担当は狭い窓でも変えられないと困るので aux ではなく children 側。
                  aux は 940px で消える */}
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>KUROOBI</span>
              <Segmented value={g.side} onChange={g.setSide} options={SIDES} />
              {/* 評価値の表示も aux ではなく children 側。**aux は 940px で
                  消えるので、狭い窓で切り替える道が無くなっていた**
                  (規則 8・58 — 担当と同じ話)。実機で 900px にすると
                  丸ごと消えることを確認した */}
              <Divider />
              <Toggle checked={g.autoHint} onChange={g.setAutoHint} label="評価値を表示" />
            </>
          )}
      </Toolbar>
        {/* 盤の上の帯は「対局の操作」と、対局の前提を決める最小限だけ。
            思考中の数字は下の帯へ */}

        {isGgs ? <GgsScreen nav={nav} snap={ggs.snap} onNav={setNav} prefs={prefs}
                       onKifu={(title, kifu, archive) => {
                         /* **番号があるなら書庫から取る。** 同期対局は
                            1 つの番号に 2 面入っていて、手元の記録は片面
                            ぶんしか無い。書庫の棋譜には評価値と消費時間も
                            付いてくる。手元の棋譜は番号が無いときの控え */
                         if (archive) {
                           setViewer({ title, kifu: '', pending: archive, archive });
                           void ggsApi.look(archive);
                         } else {
                           setViewer({ title, kifu });
                         }
                       }} />
         : isBook ? (
          bookTab === '定石' ? (
            /* 設計 §7 は 木 / 盤 / この局面 の 3 列。木は左の列が持ち、
               「この局面」と「次の手」は右のドックが持つ (幅 290 は絵の 291
               とほぼ同じ)。主画面には左メニューが乗るぶん盤は絵より狭い */
            <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
              {book.node?.size !== 0 && (
                <BookTree b={book} decimals={prefs.decimals}
                          onStudy={(kifu) => { setNav('study'); void loadFromText(kifu); }} />
              )}
              <BookPane b={book} coords={prefs.coords} grain={prefs.grain}
                        flip={flipped(prefs.facing, '')} onSettings={() => setSettings(true)} />
            </div>
          ) : (
            /* 書き戻しの明細。定石が「何にどう書き換わったか」を見る場所なので、
               木と同じ画面に置く — 対局 → 敗着 → 旧→新 → 取り消し が一本で辿れる */
            /* 設計 §8 は三面 (対局の一覧 / 敗着の盤 / この対局の明細)。
               器は幅を決めず、部品が 269 / 中央 / 290 に分ける */
            <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
              <LearnLog items={learnLog}
                onBook={(kifu) => { setBookTab('定石'); book.goto(kifu); }}
                onUndo={(e) => void (async () => {
                  if (!await confirm({
                    title: 'この対局の取り込みを取り消します',
                    body: 'この対局で書き戻した定石の値を、取り込む前に戻します。ほかの対局で'
                      + '書き戻したぶんは残ります。',
                    ok: '取り消す', danger: true,
                  })) return;
                  try {
                    await api.learnUndo(e.at, e.kifu);
                    learnLogReload();
                  } catch (err) { g.say('' + err); }
                })()}
                onOpen={(e, ply) => {
                  setNav('study');
                  void loadFromText(e.start ? e.start + '\n' + e.kifu : e.kifu, ply);
                }} />
            </div>
          )
        ) : (
        // 左右の余白は中の要素が持つ。ここに持たせると、石数の行の罫が
        // 端まで届かず途中で切れる (設計は端から端まで)
        // 下限は**この段**に置く。中の盤だけに置くと、flex は外側を伸ばし切った
        // あと (自由空間が余っている扱いのまま) 中身がはみ出して、手数の帯と
        // グラフの上に盤が重なって描かれる。ここに置くと「足りない」ことが
        // flex に伝わり、縮む側 (グラフ) から先に削られる
        <div style={{
          flex: 1, minHeight: 'calc(200px + var(--h-bar))',
          display: 'flex', flexDirection: 'column',
        }}>
          {/* 行に minmax(0,1fr) を入れて**高さを確定させる**。既定の auto だと
              行の高さが中身で決まり、中の height:100% の基準が定まらない
              (基準が無いと auto 扱いになり、svg が固有の 880px で描かれて
              下へはみ出す)。はみ出しは全部下に出るので、石数の行と手数の帯に
              重なって初めて気付く (規則 77) */}
          {/* **盤に下限を持たせる (規則 7)。** 評価値グラフの帯が縮まないので、
              窓を最小 (860×560) にすると盤が 82px まで潰れていた。畳む順は
              盤を最後まで残す決まりなので、先にグラフを縮める */}
          <div style={{
            flex: 1, minHeight: 200, display: 'grid', placeItems: 'center',
            gridTemplateRows: 'minmax(0, 1fr)', gridTemplateColumns: 'minmax(0, 1fr)',
            padding: 'var(--sp-2) var(--sp-4)',
          }}>
            {/* maxHeight も要る。器が横長のときは maxWidth が効くが、**器が低く
                  なると grid の行が中身の大きさで決まり**、svg が固有の大きさ
                  (880px) で描かれて下へはみ出す。はみ出しは全部下に出るので
                  石数の行と手数の帯に重なって初めて気付く (規則 77) */}
              <div style={{ height: '100%', maxHeight: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
              {v && <Board cells={cellsOf(v)} legal={v.legal} last={v.last} evals={evals}
                           coords={prefs.coords} grain={prefs.grain}
                           // 検討では「自分の色」が無いので、auto は黒が下のまま
                           flip={flipped(prefs.facing, study ? '' : sideColor)}
                           disabled={g.thinking}
                           onPlay={(sq) => void g.play(sq)} />}
            </div>
          </div>
          <ScoreRow black={v?.black ?? 2} white={v?.white ?? 2}
                    turn={!v || v.over ? undefined : v.player === 'black' ? 'b' : 'w'}
                    meta={study ? curMoveMeta : result}
                    /* **持ち時間があるときは残り時間**、無いときは思考の累計。
                       同じ場所に別のものが出るが、時計を付けた対局では
                       「あと何秒あるか」が読みたいもので、累計は要らない */
                    blackClock={clockLabel('b')} whiteClock={clockLabel('w')} />
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
          <EvalGraph points={povPoints} plies={v?.moves.length} cursor={v?.cursor}
                     blunder={blunder} busy={graph.busy} onJump={(n) => void g.jumpTo(n)}
                     open={graphOpen} pov={pov}
                     // n 手目の点は「n 手指し終えた局面」。指した手は n 番目
                     moveName={(n) => { const m = v?.moves[n - 1]; return m == null ? undefined : sqName(m); }}
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
                       {/* **止めるのはここ、始めるのはツールバー** (絵 §2)。
                           走っている間だけ出す — 動いていないものを止める釦を
                           描かない。始める側を帯に置いていたが、グラフが
                           畳まれている段 (620px 以下) では押す場所ごと消える */}
                       {graph.busy && (
                         <Button variant="danger" onClick={() => graph.stop()}>分析停止</Button>
                       )}
                     </>} />
        )}

      </Main>

      {/* GGS はドックを持たない (一覧が本体の左に付く) */}
      {/* 定石が無いときは空のドックも出さない (盤側が報せを出している) */}
      {isBook && bookTab === '定石' && book.node?.size !== 0 && (
        <Dock tabs={['定石']} active="定石" open={dockOpen}>
          <BookDock b={book} decimals={prefs.decimals} />
        </Dock>
      )}

      {/* 棋譜のときは丸ごとスクロールさせない — 表が列の見出しを固定し、
          行だけを流す作りになっている (操作も上に残す) */}
      {!isGgs && !isBook && (
        <PlayDock g={g} book={book} cpu={cpu} prefs={prefs} tab={tab} onTab={setTab}
                  open={dockOpen} onNav={setNav} onBookTab={setBookTab}
                  onPaste={() => setPaste(true)} onLoadFile={loadFromFile}
                  study={study} moves={moves} ggfNames={() => ggfNames(g.side)} />
      )}

      </Body>

      {/* 短くて桁の決まっているものだけを置く。長さの読めない報せはトーストへ */}
        <StatusBar
          left={<>
            {/* 分析は g.thinking を立てない (api.evalAt を局面ごとに呼ぶ) ので、
                走っている印がどこにも出ていなかった。グラフの見出し行の進み具合は
                帯の中の話で、**機械が動いているか**は下の帯が持つ (規則 11・76) */}
            {graph.busy && (
              <Busy>分析中</Busy>
            )}
            {/* 絵は「● 思考中」と秒を分けて出す。**状態と数値を 1 つの
                「思考 3.2s」に畳むと、動いているのかどうかが数字の有無でしか
                分からない** — 分析中と同じ形にして、機械が何をしているかを
                言葉で言う (規則 11) */}
            {g.thinking && (
              <Busy>思考中</Busy>
            )}
            {g.thinking && <StatusStat value={g.thinkSecs.toFixed(1)} unit="s" />}
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
            ? <>
              {/* 絵 §7 の下の帯は「登録局面 / うち学習」の 2 つ。
                  **うち学習は定石の画面のどこにも出ていなかった** —
                  対局のドックの「学習」タブにしか無く、定石を眺めている
                  ときに「この山のどれだけが実戦由来か」が読めなかった。
                  絵はツールバーにも同じ 2 つを置いているが、そちらには
                  足さない (同じ数字が 1 画面に 2 か所。要 push) */}
              <StatusStat label="登録局面"
                          value={book.node ? book.node.size.toLocaleString() : '—'} />
              <StatusStat label="うち学習"
                          value={book.node ? book.node.learned_size.toLocaleString() : '—'} />
            </>
            : <>
              {/* 検討はいま何手目を見ているかが読めないと辿れない。手数の帯の
                  目盛は 10 手ごとなので、正確な数字はここが持つ (規則 58 —
                  ツールバーは操作だけで、数値は下の帯) */}
              {study && v && <StatusStat label="棋譜" value={v.cursor} unit={`/ ${v.moves.length} 手`} />}
              {/* いまどちらから見ているか。**符号の意味は数字を見ても分からない**
                  ので、状態として出す (絵 §2 も下の帯に置いている) */}
              {study && <StatusStat value={pov === 'b' ? '黒視点' : '白視点'} />}
              <StatusStat label="定石" value={g.hasBook ? (g.useBook ? '有効' : '使わない') : 'なし'} />
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
                    // 手元の棋譜が読めないときだけ書庫へ聞き直す (1 回きり)
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

      {/* 設定。**同じ document なので値は素通りで届く** — 窓だったころは
          localStorage へ書いて `storage` の報せを待っていた */}
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

/** 環境変数の短い呼び名。**変数名をそのまま出さない** — 画面の文言は
 *  日本語で統一していて、帯だけ英語になると読み手が切り替わる。
 *  ここに無いものは `KUROOBI_` を落とした名前で出す (足し忘れても
 *  印は消えない)。 */
const ENV_LABELS: Record<string, string> = {
  KUROOBI_NO_RATED: '非レート',
  KUROOBI_GGS_DEMO: 'GGS デモ',
  KUROOBI_GGS_AUTOCONNECT: '自動接続',
  KUROOBI_GGS_AUTOVIEW: '自動表示',
  KUROOBI_GGS_AUTOWATCH: '自動観戦',
  KUROOBI_GGS_AUTOLOOK: '自動棋譜',
  KUROOBI_AUTOPLAY: '自動運転',
  KUROOBI_THEME: 'テーマ',
  KUROOBI_LEARN_LOG: '控え差替',
  KUROOBI_KEYCHAIN_SERVICE: '鍵束差替',
  KUROOBI_SESSION_LOCK: '錠差替',
  KUROOBI_WEIGHTS_DIR: '重み差替',
};

/** 左メニューの下端に出す、環境変数で起動した印。
 *
 *  **値は挙動を決めるものだけ添える。** `=1` は「立っている」の印でしか
 *  ないので出さない (`自動運転 both:14` は要るが `非レート 1` は要らない)。
 *  パスを渡す変数も値は出さない — 48px の列どころか 208px にも入らないので、
 *  **乗せたときに名前と値をそのまま出す**。
 *
 *  居場所は窓の帯の右端。**全幅なので畳む段の影響を受けず**、左メニューを
 *  48px にしても札がそのまま残る (畳んだ列用の逃げ道が要らない)。 */
function EnvTags({ items }: { items: [string, string][] }) {
  if (!items.length) return null;
  const title = items.map(([k, v]) => `${k}=${v}`).join('\n');
  return (
    <div className="k-env" title={title}>
      {items.map(([k, v]) => {
        const label = ENV_LABELS[k] ?? k.replace(/^KUROOBI_/, '');
        // 値に `/` が入るものは置き場所の差し替え。名前だけで足りる
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
