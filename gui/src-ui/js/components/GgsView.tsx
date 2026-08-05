// GGS 画面の外枠。接続状態のヘッダー・ログイン・棋譜とエンジン設定の
// モーダルを束ね、選ばれた画面に snapshot を渡す。通信は ggsApi、状態は
// snapshot だけを見る。
import { useEffect, useRef, useState } from 'react';
import { api, ggsApi } from '../api';
import type { GgsSnapshot } from '../types';
import type { View } from './Nav';
import { GgsPlay } from './GgsPlay';
import { GgsLobby } from './GgsLobby';
import { GgsUsers } from './GgsUsers';
import { GgsChat } from './GgsChat';
import { GgsStandby } from './GgsStandby';
import { GgsConsole } from './GgsConsole';
import { KifuViewer } from './KifuViewer';
import { Icon } from './Icons';
import { Strength } from './Strength';
import { Seg } from './Seg';
import { FormulaEditor } from './Formula';

/** 各画面へ渡す共通の入口。 */
export interface GgsCtx {
  snap: GgsSnapshot;
  /** プレイヤーの詳細へ移る (finger と対戦履歴を取りに行く)。 */
  showUser: (name: string) => void;
  /** 別の GGS 画面へ移る。 */
  showView: (v: View) => void;
  /** 棋譜モーダルを開く。 */
  showKifu: (title: string, text: string) => void;
  /** GGS のアーカイブから棋譜を取り出してモーダルに出す。 */
  fetchKifu: (id: string, title: string) => void;
  /** 相手を指定した状態でロビーの申し込みフォームを開く。 */
  askUser: (name: string) => void;
}

export interface GgsViewProps {
  view: View;
  onView: (v: View) => void;
  snap: GgsSnapshot | null;
  patch: (p: Partial<GgsSnapshot>) => void;
  /** 棋譜 (GGF か着手列) を検討画面で開く。 */
  onOpenStudy: (kifu: string) => void;
}

export function GgsView({ view, onView, snap, patch, onOpenStudy }: GgsViewProps) {
  const [selectedUser, setSelectedUser] = useState<string | null>(null);
  const [userTab, setUserTab] = useState<'profile' | 'history'>('profile');
  const [askOpp, setAskOpp] = useState('');
  // 棋譜モーダル。text を直接開くか、pending (アーカイブ番号) で look の
  // 結果待ちにする。結果は snapshot の fetched_ggf から表示時に導く。
  const [kifuModal, setKifuModal] =
    useState<{ title: string; text?: string; pending?: string } | null>(null);

  const fetched = snap?.fetched_ggf;
  const kifuBody = !kifuModal ? ''
    : kifuModal.pending
      ? (fetched && fetched.id === kifuModal.pending
          ? (fetched.error ? `取得できませんでした: ${fetched.error}` : fetched.ggf)
          : '取得中…')
      : kifuModal.text ?? '';

  const showUser = (name: string) => {
    setSelectedUser(name);
    setUserTab('profile');   // 前に見ていたタブを引きずらない
    onView('ggs-users');
    ggsApi.finger(name).catch(() => {});
    ggsApi.history(name === snap?.login ? '' : name).catch(() => {});
  };

  const ctx: GgsCtx | null = snap && {
    snap,
    showUser,
    showView: onView,
    showKifu: (title, text) => setKifuModal({ title, text }),
    fetchKifu: (id, title) => {
      setKifuModal({ title, pending: id });
      ggsApi.look(id).catch((e) => {
        setKifuModal({ title, text: `取得できませんでした: ${e}` });
      });
    },
    askUser: (name) => {
      setAskOpp(name);
      onView('ggs-lobby');
    },
  };

  // 画面確認用: KUROOBI_GGS_AUTOVIEW で起動直後に指定の画面を開く。
  // デモの状態は次の実イベントで上書きされる (見た目の確認専用)。
  // 一度だけ動かすタイマーなので、最新のハンドラは ref 越しに参照する。
  const autoRef = useRef<Parameters<typeof runAutoview> | null>(null);
  useEffect(() => {
    autoRef.current = [patch, onView, showUser,
      (title, text) => setKifuModal({ title, text }), () => onView('ggs-engine')];
  });
  const demoDone = useRef(false);
  useEffect(() => {
    if (demoDone.current || !snap) return;
    demoDone.current = true;
    // タイマーは一度きりなので解除しない (画面を離れていても実害がない)
    window.setTimeout(() => {
      if (autoRef.current) void runAutoview(...autoRef.current);
    }, 3000);
  }, [snap]);

  if (!snap || !ctx) {
    return <div className="ggs-root"><div className="empty">GGS セッションに接続できません。</div></div>;
  }

  return (
    <div className="ggs-root">
      <GgsHeader snap={snap} />
      {snap.conn === 'disconnected'
        ? <GgsLogin />
        : (
          <div className="ggs-body">
            {view === 'ggs-play' && <GgsPlay ctx={ctx} />}
            {view === 'ggs-lobby' && <GgsLobby ctx={ctx} initialOpp={askOpp} />}
            {view === 'ggs-users' && (
              <GgsUsers ctx={ctx} selectedUser={selectedUser}
                        onSelectUser={setSelectedUser}
                        userTab={userTab} onUserTab={setUserTab} />
            )}
            {view === 'ggs-chat' && <GgsChat ctx={ctx} />}
            {view === 'ggs-standby' && <GgsStandby ctx={ctx} />}
            {view === 'ggs-console' && <GgsConsole ctx={ctx} />}
            {view === 'ggs-engine' && <GgsEngine snap={snap} />}
          </div>
        )}

      {kifuModal && (
        <KifuViewer title={kifuModal.title} kifu={kifuBody}
                    onClose={() => setKifuModal(null)}
                    onOpenStudy={onOpenStudy}
                    onSave={(k) => void ggsApi.saveKifu(k, 'kifu').catch((e) => alert(e))} />
      )}

    </div>
  );
}

/* ---------------- ヘッダー ---------------- */

function GgsHeader({ snap }: { snap: GgsSnapshot }) {
  const r8 = snap.my_ranks.find((r) => r.gtype === '8');
  const r8r = snap.my_ranks.find((r) => r.gtype === '8r');
  return (
    <header className="ggs-header">
      <span className={'conn-dot' +
        (snap.conn === 'online' ? '' : snap.conn === 'disconnected' ? ' off' : ' mid')} />
      <span className="badge">
        {snap.conn === 'online' ? (
          <>
            <span className="k">自分</span>
            <b>{snap.login}</b>
            {r8r && <span className="rate">ランダム {r8r.rating.toFixed(0)}</span>}
            {r8 && <span className="rate">通常 {r8.rating.toFixed(0)}</span>}
          </>
        ) : snap.conn === 'disconnected' ? '未接続' : '接続中…'}
      </span>
      <span className="badge">
        <span className="k">強さ</span>
        {`深さ ${snap.engine.depth} / 読切 ${snap.engine.solve}` +
          (snap.engine.band ? ` / 選択読み +${snap.engine.band}` : '')}
      </span>
      {!snap.auto_play && <span className="badge">観戦のみ</span>}
      {snap.standby.enabled && <span className="badge gold">待機モード</span>}
      {snap.thinking && <span className="ggs-thinking">思考中…</span>}
      <span className="spacer" />
    </header>
  );
}

/* ---------------- ログイン ---------------- */

// ログインの手入力。保存済みの認証情報は起動時の自動ログインが使うので、
// この画面が出るのは未保存のとき・自動ログインに失敗したとき・
// ログアウトした後だけ。
function GgsLogin() {
  const [user, setUser] = useState('');
  const [pw, setPw] = useState('');
  const [status, setStatus] = useState('');

  const connect = async () => {
    setStatus('接続しています…');
    try {
      await ggsApi.connect(user, pw);
    } catch (e) {
      setStatus(String(e));
    }
  };

  return (
    <div className="ggs-login">
      <div className="card box">
        <h2>GGS へログイン</h2>
        <p className="hint">
          skatgame.net:5000 — ログインに成功するとキーチェーンに保存され、
          次回から自動ログインします
        </p>
        <div>
          <label className="field">ログイン名</label>
          <input type="text" value={user} onChange={(e) => setUser(e.target.value)} />
          <label className="field">パスワード</label>
          <input type="password" value={pw} onChange={(e) => setPw(e.target.value)} />
        </div>
        <button className="btn primary" onClick={() => void connect()}>ログイン</button>
        <div className="muted">{status}</div>
      </div>
    </div>
  );
}

/* ---------------- KUROOBI の設定 (GGS 用) ---------------- */

function GgsEngine({ snap }: { snap: GgsSnapshot }) {
  // 反映したことを伝える (画面なので閉じて消えることがない)
  const [saved, setSaved] = useState(false);
  const [depth, setDepth] = useState(snap.engine.depth);
  const [solve, setSolve] = useState(snap.engine.solve);
  const [band, setBand] = useState(snap.engine.band);
  const [threads, setThreads] = useState(snap.engine.threads);
  const [auto, setAuto] = useState(snap.auto_play);
  const [watch, setWatch] = useState(snap.watch_analysis);
  const [book, setBook] = useState(snap.engine.use_book);
  const [pace, setPace] = useState(snap.engine.pace || 'even');
  const [maxMove, setMaxMove] = useState(snap.engine.max_move_secs);
  const [reserve, setReserve] = useState(snap.engine.reserve_secs);
  // 並列数の選択肢を機械のコア数で切るために取る (ローカルの設定と同じ考え)
  const [cores, setCores] = useState(0);
  useEffect(() => {
    api.activity().then((a) => setCores(a.cores)).catch(() => {});
  }, []);

  // 申し込みの条件はサーバーが持っている。自分の finger を取って読み出す
  // (この画面を開いたときだけでよい — 変えるのは自分だけなので)
  const login = snap.login;
  useEffect(() => {
    if (login) ggsApi.finger(login).catch(() => {});
  }, [login]);
  /// 自分の finger から accept / decline の式を取り出す。
  const myForm = (key: 'accept' | 'decline'): string =>
    snap.fingers[login]?.fields
      .find(([k]) => k.replace(/\s+/g, '').replace(/\(.*\)/, '') === key)?.[1] ?? '';
  const save = async (kind: 'aform' | 'dform', expr: string) => {
    await ggsApi.setFormula(kind, expr).catch(() => {});
    // 送っただけで信じない。サーバーの値を取り直して画面に反映する
    if (login) ggsApi.finger(login).catch(() => {});
  };

  return (
    <div className="ggs-pane">
      <div className="pane-head">
        <h2>GGS の設定</h2>
        <p>GGS で対局・観戦するときの強さとふるまい。ローカル対局とは別に持ちます。</p>
      </div>

      <section className="pane-sec">
        <div className="pane-sec-head">
          <h3>強さ</h3>
          <p>読む深さの上限です。どこまで読めるかは下の持ち時間の使い方が決めます。</p>
        </div>
        {/* 選び方は対局側と共通 (Strength) */}
        <Strength value={{ depth, solve, band }}
                  onChange={(v) => { setDepth(v.depth); setSolve(v.solve); setBand(v.band); }} />
        <div>
          {/* コア数まで 1 刻み。飛び飛びにする理由は無い (奇数でも動くし、
              コアを 1 つ空けたいことはある)。コアを超えても食い合うだけ */}
          <label className="field">スレッド</label>
          <div className="selwrap">
            <select value={threads} onChange={(e) => setThreads(+e.target.value)}>
              {Array.from({ length: cores || threads }, (_, i) => i + 1)
                .map((n) => <option key={n} value={n}>{n}</option>)}
            </select>
          </div>
        </div>
      </section>

      <section className="pane-sec">
        <div className="pane-sec-head">
          <h3>持ち時間の使い方</h3>
          <p>1 手にかける時間を、残り時間と残り手数から決めます。時間内で
          読める深さまで読み、時間が来たらそこまでの答えで指します。</p>
        </div>
        <Seg value={pace} onChange={setPace} options={[
          ['slow', 'じっくり'], ['even', '均等'],
          ['fast', '速指し'], ['depth', '深さ固定'],
        ]} />
        <p className="hint">
          {pace === 'depth'
            ? '時間を見ずに上の深さまで読みます。持ち時間の管理は自分で行うことになります。'
            : pace === 'slow' ? '序盤に厚く配ります。研究向き。'
            : pace === 'fast' ? '序盤を短く切り上げ、終盤に残します。'
            : '残り手数で等分します。'}
        </p>
        {pace !== 'depth' && (
          <div className="num-grid">
            <div>
              <label className="field">1 手の上限 (秒、0 = なし)</label>
              <input type="number" value={maxMove}
                     onChange={(e) => setMaxMove(Math.max(0, +e.target.value))} />
            </div>
            <div>
              <label className="field">読み切り用に残す (秒)</label>
              <input type="number" value={reserve}
                     onChange={(e) => setReserve(Math.max(0, +e.target.value))} />
            </div>
          </div>
        )}
      </section>

      <section className="pane-sec">
        <div className="pane-sec-head"><h3>定石</h3></div>
        {snap.engine.book_loaded ? (
          <Seg value={book ? 'on' : 'off'} onChange={(v) => setBook(v === 'on')}
               options={[['on', '使う'], ['off', '使わない']] as const} />
        ) : (
          <p className="warn-line">
            <Icon name="alert" size={14} />
            ファイルがありません。左メニュー下の「設定」で指定してください。
          </p>
        )}
      </section>

      <section className="pane-sec">
        <div className="pane-sec-head">
          <h3>申し込みの扱い</h3>
          <p>相手から対局を申し込まれたときに、自動で受ける / 断る条件です。
          <b>サーバー側に残る</b>ので、アプリを閉じていても効きます。
          受ける条件と断る条件の両方に当てはまるときは、断るほうが勝ちます。</p>
        </div>
        <div>
          <label className="field">自動で受ける条件</label>
          <FormulaEditor value={myForm('accept')}
                         onSave={(s) => void save('aform', s)} />
        </div>
        <div>
          <label className="field">自動で断る条件</label>
          <FormulaEditor value={myForm('decline')}
                         onSave={(s) => void save('dform', s)} />
        </div>
      </section>

      <section className="pane-sec">
        <div className="pane-sec-head"><h3>ふるまい</h3></div>
        <label className="check">
          <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
          自分の手番で自動的に指す
        </label>
        <label className="check">
          <input type="checkbox" checked={watch} onChange={(e) => setWatch(e.target.checked)} />
          観戦中の対局も解析する
        </label>
      </section>

      <div className="pane-actions">
        {saved && <span className="saved-note"><Icon name="check" size={14} />反映しました</span>}
        <span className="spacer" />
        <button className="btn primary" onClick={async () => {
          await ggsApi.setEngine(depth, solve, band, threads).catch(() => {});
          await ggsApi.setPacing(pace, maxMove, reserve).catch(() => {});
          await ggsApi.setAutoPlay(auto).catch(() => {});
          await ggsApi.setWatchAnalysis(watch).catch(() => {});
          await ggsApi.setUseBook(book).catch(() => {});
          setSaved(true);
          window.setTimeout(() => setSaved(false), 2000);
        }}>適用</button>
      </div>

      <section className="pane-sec danger-sec">
        <div className="pane-sec-head">
          <h3>接続</h3>
          <p>ログアウトすると保存済みの認証情報も消えます。次の起動では自動
          ログインしません。</p>
        </div>
        <div>
          <button className="btn danger" onClick={() => void ggsApi.disconnect()}>
            <Icon name="logout" size={15} />ログアウト
          </button>
        </div>
      </section>
    </div>
  );
}

/* ---------------- 画面確認用のデモ (KUROOBI_GGS_AUTOVIEW) ---------------- */

/** デモの盤に並べる実戦の手順 (60 手で埋まる)。 */
const DEMO_KIFU =
  'e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2'
  + 'a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1';

const DIRS: [number, number][] = [
  [-1, -1], [0, -1], [1, -1], [-1, 0], [1, 0], [-1, 1], [0, 1], [1, 1],
];

/** 棋譜を n 手まで並べた盤面 (0 空 / 1 黒 / 2 白)。file-major。
 *
 * 打てない手番はパスとして飛ばす。デモだからと石を機械的に置くと、
 * 挟まれていないのに色が違う・石が飛び地になるといった、ありえない配置が
 * そのまま画面に出る。 */
function replay(kifu: string, n: number): number[] {
  const c = new Array(64).fill(0);
  c[3 * 8 + 3] = 2; c[4 * 8 + 3] = 1; c[3 * 8 + 4] = 1; c[4 * 8 + 4] = 2;
  const flips = (f: number, r: number, me: number): number[] => {
    if (c[f * 8 + r] !== 0) return [];
    const got: number[] = [];
    for (const [df, dr] of DIRS) {
      const line: number[] = [];
      let x = f + df, y = r + dr;
      while (x >= 0 && x < 8 && y >= 0 && y < 8 && c[x * 8 + y] === 3 - me) {
        line.push(x * 8 + y);
        x += df; y += dr;
      }
      if (line.length && x >= 0 && x < 8 && y >= 0 && y < 8 && c[x * 8 + y] === me) {
        got.push(...line);
      }
    }
    return got;
  };
  let me = 1;
  for (let i = 0; i < n; i++) {
    const mv = kifu.slice(i * 2, i * 2 + 2);
    if (mv.length < 2) break;
    const f = mv.charCodeAt(0) - 97;
    const r = mv.charCodeAt(1) - 49;
    let got = flips(f, r, me);
    if (!got.length) { me = 3 - me; got = flips(f, r, me); }   // 相手がパス
    if (!got.length) break;                                    // 棋譜が合わない
    c[f * 8 + r] = me;
    for (const p of got) c[p] = me;
    me = 3 - me;
  }
  return c;
}

/** 棋譜表に出す着手の並び (n 手ぶん)。 */
const demoMoves = (n: number): string[] =>
  Array.from({ length: n }, (_, i) => DEMO_KIFU.slice(i * 2, i * 2 + 2).toUpperCase());

async function runAutoview(
  patch: (p: Partial<GgsSnapshot>) => void,
  onView: (v: View) => void,
  showUser: (name: string) => void,
  showKifu: (title: string, text: string) => void,
  openEngine: () => void,
): Promise<void> {
  const v = await ggsApi.autoview().catch(() => '');
  if (!v) return;
  const [tab, arg] = v.split(':');
  if (tab === 'playdemo') {
    // 見た目の確認用: 手合いの一覧と盤を、通信せずに埋める
    // 実在する進行を並べる。適当に石を置くと、盤にありえない配置が出る
    // (石が飛び地になる・挟まれていないのに色が違う、など)
    const cells = (n: number): number[] => replay(DEMO_KIFU, n);
    const p = (name: string, rating: string, color: string) =>
      ({ name, rating, color, clock: '12:30', secs: 750, ext: 30 });
    const mk = (id: string, base: string, opp: string, my: string, n: number,
                gt: string, players: ReturnType<typeof p>[]) => ({
      id, base, cells: cells(n), turn: 'black', my_color: my,
      opp_name: opp, opp_rating: '2612.4', opp_clock: '12:30', my_clock: '11:04',
      my_secs: 664, opp_secs: 750, my_ext: 30, opp_ext: 30,
      players, gtype: gt, moves: demoMoves(n), ggf: '',
      last_eval: my ? 2.5 : null, last_eval_exact: false, last_from_book: true,
      watch_eval: my ? null : -1.5, watch_best: my ? null : 'D6', watch_exact: false, seen: n,
    });
    patch({
      matches: [
        mk('.71.0', '.71', 'demo-bob', 'black', 23, 's8r16', []),
        mk('.71.1', '.71', 'demo-bob', 'white', 22, 's8r16', []),
        mk('.80.0', '.80', '', '', 31, 's8r14',
           [p('demo-carol', '2246.5', 'black'), p('demo-dave', '2198.3', 'white')]),
        mk('.80.1', '.80', '', '', 30, 's8r14',
           [p('demo-dave', '2198.3', 'black'), p('demo-carol', '2246.5', 'white')]),
        mk('.90', '.90', '', '', 44, '8',
           [p('demo-erin', '1938.0', 'black'), p('demo-frank', '1687.7', 'white')]),
      ] as unknown as GgsSnapshot['matches'],
      thinking: '.71.0',
      conn: 'online',
      login: 'kuroobi',
    });
    onView('ggs-play');
    return;
  }
  if (tab === 'chatdemo') {
    // 見た目の確認用: 実際に発言せずにチャット画面を埋める
    const now = Math.floor(Date.now() / 1000);
    patch({
      conn: 'online',
      login: 'kuroobi',
      chat: [
        { chan: '.chat', from: 'demo-bob', text: 'anyone up for a game?', at: now - 3600, thread: '.chat' },
        { chan: '.chat', from: 'kuroobi', text: 'sure, 15 min?', at: now - 3550, thread: '.chat' },
        { chan: '.chat', from: 'demo-bob', text: 'sounds good. synchro rand 16?', at: now - 3500, thread: '.chat' },
        { chan: '.chat', from: 'kuroobi', text: 'ok, sending a request now', at: now - 3400, thread: '.chat' },
        { chan: '.chat', from: 'demo-carol', text: 'good luck both', at: now - 900, thread: '.chat' },
        { chan: '', from: 'demo-erin', text: 'nice endgame in that last one', at: now - 300, thread: 'demo-erin' },
        { chan: '→demo-erin', from: 'kuroobi', text: 'thanks! the book helped a lot', at: now - 120, thread: 'demo-erin' },
        { chan: '', from: 'demo-erin', text: 'rematch later?', at: now - 30, thread: 'demo-erin' },
      ],
    });
    onView('ggs-chat');
    return;
  }
  if (tab === 'tour') {
    // 画面確認用: 各画面を順に開く。外から撮って見比べるため。
    // 未接続のままだと GGS はどの画面もログインを出すので、繋がったことに
    // しておく (中身は空でよい — 見たいのは画面の枠組み)
    patch({ conn: 'online', login: 'kuroobi' });
    const tabs: View[] = ['ggs-lobby', 'ggs-users', 'ggs-standby', 'ggs-console', 'ggs-engine'];
    let i = 0;
    onView(tabs[0]);
    setInterval(() => { i++; onView(tabs[i % tabs.length]); }, 9000);
    return;
  }
  if (tab === 'userdemo') {
    // 画面確認用: プロフィールを通信せずに埋める。finger の生の並びをそのまま
    // 置く (項目の分類・条件式の入れ子・2 列の折り方を確かめるため)。
    // 条件式は実際に GGS で見かける形をそのまま使っている
    // showUser は finger と history を取りに行き、未接続だと失敗して
    // スナップショットを押し戻す。デモの値はその後に載せる
    const demo = {
      conn: 'online' as const, login: 'kuroobi',
      users: [{ name: 'demo-scorpion', rating: 2245.8, raw: '2245.8@ 34.2' }],
      fingers: {
        'demo-scorpion': {
          name: 'demo-scorpion',
          raw: [],
          fields: [
            ['open', '+'],
            ['accept', ': rand & discs>=14 & discs<=20 & mt1>=120'],
            ['decline', ': !saved & (size!=8 | anti | mc!=? | (rand & (discs<14 | discs>20))'
              + ' | (synchro & !rand) | mt1<60 | mm1!=0 | ml1==F | ot1>1800 | stored>0) | !rated'],
            ['rated', '+'], ['play', '-'], ['stored (-)', ''],
            ['name', '6-ply LOGISTELLO'], ['since', 'Thu 31 Jul 2026 08:39:06 MDT'],
            ['idle', '00:01:12, on line : 5.13:32:08'], ['host', 'example.net'],
            ['dblen', '100.0 = 2,862 / 2,862'],
            ['level', '1'], ['trust', '+'], ['client', '+'], ['vt100', '-'], ['hear', '+'],
            ['bell', '-r -p -w -n'], ['groups (+)', '_client'], ['notify (+)', '/os'],
            ['channs (+)', ''], ['watch (-)', ''], ['track (-)', ''], ['ignore (-)', ''],
          ] as [string, string][],
        },
      },
    };
    patch(demo);
    showUser('demo-scorpion');
    return;
  }
  if (tab === 'user' && arg) {
    showUser(arg);
    return;
  }
  if (tab === 'formdemo') {
    // 画面確認用: 申し込みの条件を組む節を、通信せずに開く
    patch({
      conn: 'online', login: 'kuroobi',
      fingers: {
        kuroobi: {
          name: 'kuroobi', raw: [],
          fields: [
            ['accept', ': rand & discs>=14 & mt1>=120'],
            ['decline', ': !saved & (size!=8 | anti) | !rated'],
          ] as [string, string][],
        },
      },
    });
    openEngine();
    // 条件の節は下のほうにある。確認用なのでそこまで送る
    setTimeout(() => document.querySelector('.ggs-pane')?.scrollTo(0, 99999), 400);
    return;
  }
  if (tab === 'engine') { openEngine(); return; }
  if (tab === 'kifu') {
    showKifu('kuroobi 対 demo-erin',
      '(;GM[Othello]PB[kuroobi]PW[demo-erin]RE[+14.00]'
      + 'BO[8 ---------------------------O*------*O--------------------------- *]'
      // 実際に打てる手順にしておく (盤面で再生するので、不正だと開けない)
      + 'B[F5]W[D6/-6.46]B[C3]W[D3/-16.49]B[C4]W[F4/-8.10];)');
    return;
  }
  const known: View[] = ['ggs-play', 'ggs-lobby', 'ggs-users', 'ggs-chat', 'ggs-standby', 'ggs-console'];
  const target = ('ggs-' + tab) as View;
  if (known.includes(target)) onView(target);
}
