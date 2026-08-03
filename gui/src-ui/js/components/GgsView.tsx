// GGS 画面の外枠。接続状態のヘッダー・ログイン・棋譜とエンジン設定の
// モーダルを束ね、選ばれた画面に snapshot を渡す。通信は ggsApi、状態は
// snapshot だけを見る。
import { useEffect, useRef, useState } from 'react';
import { ggsApi } from '../api';
import type { GgsSnapshot } from '../types';
import type { View } from './Nav';
import { GgsPlay } from './GgsPlay';
import { GgsLobby } from './GgsLobby';
import { GgsUsers } from './GgsUsers';
import { GgsChat } from './GgsChat';
import { GgsStandby } from './GgsStandby';
import { GgsConsole } from './GgsConsole';
import { Modal } from './Modal';
import { Icon } from './Icons';

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
        <Modal title={`棋譜 — ${kifuModal.title}`} onClose={() => setKifuModal(null)}
               actions={<>
                 <button className="btn ghost" onClick={() => setKifuModal(null)}>閉じる</button>
                 <span className="spacer" />
                 <CopyButton text={kifuBody} />
                 <button className="btn" onClick={() =>
                   void ggsApi.saveKifu(kifuBody, 'kifu').catch((e) => alert(e))}>
                   ファイルに保存
                 </button>
                 <button className="btn primary" onClick={() => {
                   onOpenStudy(kifuBody);
                   setKifuModal(null);
                 }}>検討で開く</button>
               </>}>
          <textarea readOnly value={kifuBody || '(手なし)'} />
        </Modal>
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

/* ---------------- コピー (押した直後だけ文言を変える) ---------------- */

function CopyButton({ text }: { text: string }) {
  const [done, setDone] = useState(false);
  return (
    <button className="btn" onClick={async () => {
      try { await navigator.clipboard.writeText(text); } catch { /* 選択不可環境 */ }
      setDone(true);
      setTimeout(() => setDone(false), 1200);
    }}>{done ? 'コピーしました' : 'コピー'}</button>
  );
}

/* ---------------- KUROOBI の設定 (GGS 用) ---------------- */

const PRESETS: Record<string, [number, number, number, number]> = {
  // [中盤の深さ, 読切 (空き), 選択読み, スレッド]
  // 実戦は GGS の持ち時間 (15 分前後) で使い切らない設定。最強は時間を
  // かけてよい場面向けで、実戦より必ず深い。
  light: [12, 18, 0, 2],
  ggs: [22, 26, 6, 4],
  max: [26, 30, 10, 8],
};

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
  const [preset, setPreset] = useState<string>(() => {
    const hit = Object.entries(PRESETS).find(([, p]) =>
      p[0] === snap.engine.depth && p[1] === snap.engine.solve && p[2] === snap.engine.band);
    return hit?.[0] ?? 'custom';
  });

  const applyPreset = (name: string) => {
    setPreset(name);
    const p = PRESETS[name];
    if (p) {
      setDepth(p[0]); setSolve(p[1]); setBand(p[2]); setThreads(p[3]);
    }
  };

  const num = (v: number, set: (n: number) => void) => (
    <input type="number" value={v} onChange={(e) => { set(+e.target.value); setPreset('custom'); }} />
  );

  return (
    <div className="ggs-pane">
      <div className="pane-head">
        <h2>GGS の設定</h2>
        <p>GGS で対局・観戦するときの強さとふるまい。ローカル対局とは別に持ちます。</p>
      </div>

      <section className="pane-sec">
        <div className="sec-head">
          <h3>強さ</h3>
          <p>持ち時間が減ると、この設定を上限に自動で浅くします。</p>
        </div>
        <div className="seg">
          {[['light', '軽量'], ['ggs', '実戦'], ['max', '最強'], ['custom', '自由設定']].map(([v, label]) => (
            <button key={v} className={preset === v ? 'active' : ''}
                    onClick={() => applyPreset(v)}>{label}</button>
          ))}
        </div>
        <div className="num-grid">
          <div><label className="field">中盤の深さ</label>{num(depth, setDepth)}</div>
          <div><label className="field">読切 (空き)</label>{num(solve, setSolve)}</div>
          <div><label className="field">選択読み</label>{num(band, setBand)}</div>
          <div><label className="field">スレッド</label>{num(threads, setThreads)}</div>
        </div>
      </section>

      <section className="pane-sec">
        <div className="sec-head"><h3>定石</h3></div>
        {snap.engine.book_loaded ? (
          <div className="seg">
            <button className={book ? 'active' : ''} onClick={() => setBook(true)}>使う</button>
            <button className={book ? '' : 'active'} onClick={() => setBook(false)}>使わない</button>
          </div>
        ) : (
          <p className="warn-line">
            <Icon name="alert" size={14} />
            ファイルがありません。左メニュー下の「設定」で指定してください。
          </p>
        )}
      </section>

      <section className="pane-sec">
        <div className="sec-head"><h3>ふるまい</h3></div>
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
          await ggsApi.setAutoPlay(auto).catch(() => {});
          await ggsApi.setWatchAnalysis(watch).catch(() => {});
          await ggsApi.setUseBook(book).catch(() => {});
          setSaved(true);
          window.setTimeout(() => setSaved(false), 2000);
        }}>適用</button>
      </div>

      <section className="pane-sec danger-sec">
        <div className="sec-head">
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
    const cells = (moves: number): number[] => {
      const c = new Array(64).fill(0);
      const put = (f: number, r: number, x: number) => { c[f * 8 + r] = x; };
      put(3, 3, 2); put(4, 3, 1); put(3, 4, 1); put(4, 4, 2);
      for (let i = 0; i < moves; i++) put(i % 8, (i * 3) % 8, (i % 2) + 1);
      return c;
    };
    const p = (name: string, rating: string, color: string) =>
      ({ name, rating, color, clock: '12:30', secs: 750, ext: 30 });
    const mk = (id: string, base: string, opp: string, my: string, n: number,
                gt: string, players: ReturnType<typeof p>[]) => ({
      id, base, cells: cells(n), turn: 'black', my_color: my,
      opp_name: opp, opp_rating: '2612.4', opp_clock: '12:30', my_clock: '11:04',
      my_secs: 664, opp_secs: 750, my_ext: 30, opp_ext: 30,
      players, gtype: gt, moves: new Array(n).fill('f5'), ggf: '',
      last_eval: my ? 2.5 : null, last_eval_exact: false, last_from_book: true,
      watch_eval: my ? null : -1.5, watch_best: my ? null : 'D6', watch_exact: false, seen: n,
    });
    patch({
      matches: [
        mk('.71.0', '.71', 'Rhapsody', 'black', 23, 's8r16', []),
        mk('.71.1', '.71', 'Rhapsody', 'white', 22, 's8r16', []),
        mk('.80.0', '.80', '', '', 31, 's8r14',
           [p('nyanyan', '2646.5', 'black'), p('egrcd', '2598.3', 'white')]),
        mk('.80.1', '.80', '', '', 30, 's8r14',
           [p('egrcd', '2598.3', 'black'), p('nyanyan', '2646.5', 'white')]),
        mk('.90', '.90', '', '', 44, '8',
           [p('scorpion', '1938.0', 'black'), p('viper', '1687.7', 'white')]),
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
        { chan: '.chat', from: 'Rhapsody', text: 'anyone up for a game?', at: now - 3600, thread: '.chat' },
        { chan: '.chat', from: 'kuroobi', text: 'sure, 15 min?', at: now - 3550, thread: '.chat' },
        { chan: '.chat', from: 'Rhapsody', text: 'sounds good. synchro rand 16?', at: now - 3500, thread: '.chat' },
        { chan: '.chat', from: 'kuroobi', text: 'ok, sending a request now', at: now - 3400, thread: '.chat' },
        { chan: '.chat', from: 'nyanyan', text: 'good luck both', at: now - 900, thread: '.chat' },
        { chan: '', from: 'scorpion', text: 'nice endgame in that last one', at: now - 300, thread: 'scorpion' },
        { chan: '→scorpion', from: 'kuroobi', text: 'thanks! the book helped a lot', at: now - 120, thread: 'scorpion' },
        { chan: '', from: 'scorpion', text: 'rematch later?', at: now - 30, thread: 'scorpion' },
      ],
    });
    onView('ggs-chat');
    return;
  }
  if (tab === 'tour') {
    // 画面確認用: 各画面を順に開く。外から撮って見比べるため。
    const tabs: View[] = ['ggs-users', 'ggs-standby', 'ggs-console'];
    let i = 0;
    onView(tabs[0]);
    setInterval(() => { i++; onView(tabs[i % tabs.length]); }, 9000);
    return;
  }
  if (tab === 'user' && arg) {
    showUser(arg);
    return;
  }
  if (tab === 'engine') { openEngine(); return; }
  if (tab === 'kifu') {
    showKifu('kuroobi 対 scorpion',
      '(;GM[Othello]PB[kuroobi]PW[scorpion]RE[+14.00]'
      + 'BO[8 ---------------------------O*------*O--------------------------- *]'
      + 'B[E8]W[d2/-6.46]B[C8]W[b7/-16.49];)');
    return;
  }
  const known: View[] = ['ggs-play', 'ggs-lobby', 'ggs-users', 'ggs-chat', 'ggs-standby', 'ggs-console'];
  const target = ('ggs-' + tab) as View;
  if (known.includes(target)) onView(target);
}
