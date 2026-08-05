// プレイヤー。自分のレート・接続中/ランキングの一覧・プレイヤー詳細
// (プロフィールと対戦履歴)。GGS の記号をそのまま出しても読めないので、
// finger の項目と条件式は日本語に読み下す。
import { useState } from 'react';
import { ggsApi } from '../api';
import type { GgsSnapshot } from '../types';
import { gtypeLabel, readFormula } from '../ggs';
import type { GgsCtx } from './GgsView';

export interface GgsUsersProps {
  ctx: GgsCtx;
  selectedUser: string | null;
  onSelectUser: (name: string | null) => void;
  userTab: 'profile' | 'history';
  onUserTab: (t: 'profile' | 'history') => void;
}

export function GgsUsers(props: GgsUsersProps) {
  const { ctx, selectedUser } = props;
  return (
    <div className="ggs-cols">
      {selectedUser
        ? <UserDetail {...props} name={selectedUser} />
        : <UserList ctx={ctx} />}
    </div>
  );
}

/* ---------------- 一覧 ---------------- */

function UserList({ ctx }: { ctx: GgsCtx }) {
  const { snap } = ctx;
  const [mode, setMode] = useState<'who' | 'top'>('who');
  const [pool, setPool] = useState('8');
  const [page, setPage] = useState(0);
  const [perPage, setPerPage] = useState(25);

  const refresh = (m = mode, p = pool) => {
    setPage(0);
    if (m === 'who') void ggsApi.who(p);
    else void ggsApi.top(p, 100);
  };

  const rows = mode === 'who' ? snap.users : snap.ranking;
  const pages = Math.max(1, Math.ceil(rows.length / perPage));
  const cur = Math.min(page, pages - 1);
  const slice = rows.slice(cur * perPage, (cur + 1) * perPage);

  return (
    <div className="col-main">
      <div className="sec">
        <div className="sec-head">
          <h2>自分のレート</h2>
          <button className="btn small" onClick={() => {
            for (const t of ['8', '8r']) void ggsApi.rank(t, snap.login);
          }}>更新</button>
        </div>
        <MyRanks snap={snap} />
      </div>

      <div className="sec grow">
        <div className="sec-head">
          <div className="seg">
            <button className={mode === 'who' ? 'active' : ''}
                    onClick={() => { setMode('who'); refresh('who'); }}>接続中</button>
            <button className={mode === 'top' ? 'active' : ''}
                    onClick={() => { setMode('top'); refresh('top'); }}>ランキング</button>
          </div>
          <div className="seg">
            <button className={pool === '8' ? 'active' : ''}
                    onClick={() => { setPool('8'); refresh(mode, '8'); }}>通常</button>
            <button className={pool === '8r' ? 'active' : ''}
                    onClick={() => { setPool('8r'); refresh(mode, '8r'); }}>ランダム</button>
          </div>
          <span className="muted">{rows.length} 人</span>
          <button className="btn small" onClick={() => refresh()}>更新</button>
        </div>
        <div className="scroll grow">
          <table className="users-table">
            <thead>
              <tr><th className="num">#</th><th>名前</th><th>レート</th><th>状態</th></tr>
            </thead>
            <tbody>
              {slice.map((u, i) => {
                const idx = cur * perPage + i + 1;
                const m = /(\d+(?:\.\d+)?)@\s*(\d+(?:\.\d+)?)/.exec(u.raw || '');
                const dev = m ? `±${Math.round(parseFloat(m[2]))}` : '';
                const playing = snap.ongoing.some((o) => o.names.includes(u.name));
                return (
                  <tr key={u.name} className={u.name === snap.login ? 'me' : ''}
                      onClick={() => ctx.showUser(u.name)}>
                    <td className="num">{idx}</td>
                    <td className="nowrap">{u.name}</td>
                    <td className="rating">
                      {u.rating != null ? u.rating.toFixed(1) : '—'}
                      {dev && <span className="dev">{dev}</span>}
                    </td>
                    <td>
                      <span className={'state ' + (playing ? 'playing' : 'idle')}>
                        {playing ? '対局中' : '待機'}
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        <div className="pager">
          <button className="btn small" disabled={cur === 0}
                  onClick={() => setPage(cur - 1)}>前へ</button>
          <span className="muted">{cur + 1} / {pages}</span>
          <button className="btn small" disabled={cur >= pages - 1}
                  onClick={() => setPage(cur + 1)}>次へ</button>
          <span className="spacer" />
          <span className="muted">表示件数</span>
          <div className="selwrap fix" style={{ width: 88 }}>
            <select value={perPage}
                    onChange={(e) => { setPerPage(+e.target.value); setPage(0); }}>
              {[25, 50, 100].map((n) => <option key={n} value={n}>{n}</option>)}
            </select>
          </div>
        </div>
      </div>
    </div>
  );
}

function MyRanks({ snap }: { snap: GgsSnapshot }) {
  const rows = snap.my_ranks;
  if (!rows.length) return <div className="empty">接続すると表示されます。</div>;
  const label: Record<string, string> = { '8': '通常開局', '8r': 'ランダム開局' };
  return (
    <div>
      {rows.map((r) => {
        const games = r.wins + r.draws + r.losses;
        return (
          <div key={r.gtype} className="rank-row">
            <div className="rank-pool">{label[r.gtype] || r.gtype}</div>
            <div className="rank-value">
              {r.rating.toFixed(1)}
              <span className="dev">±{r.dev.toFixed(0)}{r.dev >= 100 ? ' 暫定' : ''}</span>
            </div>
            <div className="rank-meta">
              <span className="muted">{r.rank} 位</span>
              {games > 0 && (
                <span>
                  <span style={{ color: 'var(--ok)' }}>{r.wins}勝</span>{' '}
                  <span style={{ color: 'var(--bad)' }}>{r.losses}敗</span>{' '}
                  {r.draws}分
                </span>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

/* ---------------- 詳細 ---------------- */

function UserDetail({ ctx, name, onSelectUser, userTab, onUserTab }:
    GgsUsersProps & { name: string }) {
  const { snap } = ctx;
  return (
    <div className="col-main">
      <div className="sec grow">
        <div className="sec-head">
          <button className="btn ghost icon" title="一覧へ戻る"
                  onClick={() => onSelectUser(null)}>←</button>
          <h2>{name}</h2>
          <span className="spacer" />
          <button className="btn primary"
                  onClick={() => ctx.askUser(name)}>対局を申し込む</button>
        </div>
        <div className="seg">
          <button className={userTab === 'profile' ? 'active' : ''}
                  onClick={() => onUserTab('profile')}>プロフィール</button>
          <button className={userTab === 'history' ? 'active' : ''}
                  onClick={() => onUserTab('history')}>対戦履歴</button>
        </div>
        <div className="scroll grow">
          {userTab === 'profile'
            ? <Profile snap={snap} name={name} />
            : <History ctx={ctx} name={name} />}
        </div>
      </div>
    </div>
  );
}

const FINGER_LABEL: Record<string, string> = {
  // --- 対局を申し込む前に見たいもの ---
  open: '申し込み受付', accept: '自動で受ける条件', decline: '自動で断る条件',
  'request(+)': '募集中の条件', 'request(-)': '募集中の条件',
  rated: 'レート戦', play: '対局の状況',
  'stored(+)': '中断中の対局', 'stored(-)': '中断中の対局',
  // --- 素性 ---
  name: '登録名', info: '備考', email: 'メール', since: '接続開始',
  idle: '無操作の時間', host: 'ホスト', dblen: '定跡どおりの手',
  // --- 設定・状態 ---
  level: 'アクセス権限', trust: '信用', client: 'クライアント',
  sock: '接続方式', bell: '通知を受け取るもの', hear: '発言の受信', vt100: 'VT100 表示',
  'watch(+)': '観戦中の対局', 'watch(-)': '観戦中の対局',
  'track(+)': '入退室を知らせる相手', 'track(-)': '入退室を知らせる相手',
  'groups(+)': '所属グループ', 'groups(-)': '所属グループ',
  'channs(+)': '参加チャンネル', 'channs(-)': '参加チャンネル',
  'notify(+)': '通知を受け取る相手', 'notify(-)': '通知を受け取る相手',
  'ignore(+)': '無視している相手', 'ignore(-)': '無視している相手',
};
// 画面で意味を持たないもの (認証情報と、コマンドのエコー)
const FINGER_HIDDEN = ['passw', 'password', 'login', '/os', 'sock'];
// 対局を申し込む前に見たい項目。値が空でも「指定なし」と出す。
const FINGER_ALWAYS = ['open', 'accept', 'decline', 'request(+)', 'request(-)'];
const FINGER_ORDER = [
  // 対局を申し込む前に見たい順に並べる
  'open', 'accept', 'decline', 'request(+)', 'request(-)', 'rated', 'play',
  'name', 'info', 'email', 'since', 'idle', 'host', 'dblen',
];

/// finger のキーは "stored (-)" のように空白が入ることがある。
const normKey = (k: string): string => k.replace(/\s+/g, '');

// GGS の記号をそのまま出しても読めないので言い換える。
function fingerValue(k: string, v: string): string {
  const key = normKey(k).replace(/\(.*\)/, '');
  if (key === 'open') return v === '0' || v === '-' ? '受け付けていない' : '受け付ける';
  if (key === 'rated' || key === 'trust') return v === '+' ? 'あり' : 'なし';
  if (key === 'accept' || key === 'decline' || key === 'request') {
    const body = v.replace(/^\s*:\s*/, '').trim();
    return body ? readFormula(body) : '指定なし (申し込みごとに本人が判断)';
  }
  if (key === 'notify') return v === '/os' ? 'リバーシサービス全体' : (v.trim() || 'なし');
  if (key === 'play') return v === '-' || !v ? '対局していない' : `対局中 (${v})`;
  if (key === 'client') return v === '+' ? '専用クライアント' : 'telnet など';
  if (key === 'hear') return v === '+' ? '受け取る' : '受け取らない';
  if (key === 'vt100') return v === '+' ? '対応' : '非対応';
  if (key === 'level') return v === '1' ? '一般' : v;
  if (key === 'dblen') {
    // "100.0 = 2,862 / 2,862" = 公開棋譜データベースと一致した手の割合
    const m = /([\d.]+)\s*=\s*([\d,]+)\s*\/\s*([\d,]+)/.exec(v);
    return m ? `${m[1]}% (${m[2]} / ${m[3]} 手が一致)` : v;
  }
  if (key === 'groups') return v === '_client' ? 'クライアント (対局プログラム)' : v;
  if (key === 'bell') return readBell(v);
  if (key === 'since' || key === 'idle') return readTime(k, v);
  if (['track', 'watch', 'groups', 'channs', 'notify', 'ignore', 'stored'].includes(key)) {
    return v.trim() || 'なし';
  }
  return v;
}

/// 通知設定 (`-r -p -w ...`) は記号の羅列なので、有効なものだけ並べる。
const BELL_LABEL: Record<string, string> = {
  r: '対局の申し込み', p: '個人あての発言', w: '観戦中の対局', n: 'お知らせ',
  ns: '対局開始', nn: '新しい対局', nt: '手番', ni: '中断', nr: '再開', nw: '観戦',
  ta: '全体の発言', to: '対局中の発言', tp: '個人あて',
};
function readBell(v: string): string {
  const on = v.split(/\s+/).filter((t) => t.startsWith('+')).map((t) => t.slice(1));
  const names = on.map((k) => BELL_LABEL[k] || k).filter(Boolean);
  return names.length ? names.join('、') : 'すべて切っている';
}

/// GGS の時刻表記を日本語のロケールに直す。
/// `since` は "Thu 30 Jul 2026 17:39:06 MDT"、`idle` は "00:14:02, on line : 1.09:59:08"。
function readTime(k: string, v: string): string {
  if (k === 'since') {
    const t = Date.parse(v.replace(/\s*[A-Z]{3}$/, ' GMT-0600'));
    if (!Number.isNaN(t)) {
      return new Date(t).toLocaleString('ja-JP', {
        year: 'numeric', month: 'long', day: 'numeric',
        hour: '2-digit', minute: '2-digit', weekday: 'short',
      });
    }
    return v;
  }
  // idle: 手前が無操作の時間、"on line" が接続してからの時間
  const m = /^([\d:.]+)(?:,\s*on line\s*:\s*([\d:.]+))?/.exec(v.trim());
  if (!m) return v;
  const span = (x: string): string => {
    const [d, hms] = x.includes('.') ? x.split('.') : ['0', x];
    const [h, mi] = hms.split(':').map(Number);
    const parts: string[] = [];
    if (+d) parts.push(`${+d} 日`);
    if (h) parts.push(`${h} 時間`);
    parts.push(`${mi ?? 0} 分`);
    return parts.join(' ');
  };
  const idle = span(m[1]);
  return m[2] ? `${idle} (接続してから ${span(m[2])})` : idle;
}

/// 条件式は 1 文が長い。最上位の「または」で折って箇条書きにする。
function FormulaCell({ k, text }: { k: string; text: string }) {
  const key = normKey(k).replace(/\(.*\)/, '');
  const isFormula = ['accept', 'decline', 'request'].includes(key);
  if (!isFormula || text.length < 60) return <div className="v">{text}</div>;
  // 括弧の外にある「または」だけで割る
  const parts: string[] = [];
  let depth = 0;
  let buf = '';
  const toks = text.split(/(\(|\)|\sまたは\s)/);
  for (const t of toks) {
    if (t === '(') depth++;
    if (t === ')') depth--;
    if (depth === 0 && /^\s*または\s*$/.test(t)) { parts.push(buf.trim()); buf = ''; continue; }
    buf += t;
  }
  if (buf.trim()) parts.push(buf.trim());
  if (parts.length < 2) return <div className="v">{text}</div>;
  return (
    <div className="v">
      <div className="muted small">次のどれかに当てはまるとき:</div>
      <ul className="formula">
        {parts.map((p, i) => <li key={i}>{p}</li>)}
      </ul>
    </div>
  );
}

function Profile({ snap, name }: { snap: GgsSnapshot; name: string }) {
  const f = snap.fingers[name];
  if (!f) return <div className="empty">取得中…</div>;
  const shown = f.fields.filter(([k, v]) =>
    (v || FINGER_ALWAYS.includes(k))
    && !FINGER_HIDDEN.includes(k.toLowerCase().replace(/\(.*\)/, '')));
  shown.sort((a, b) => {
    const ra = FINGER_ORDER.indexOf(a[0]);
    const rb = FINGER_ORDER.indexOf(b[0]);
    return (ra < 0 ? 99 : ra) - (rb < 0 ? 99 : rb);
  });
  return (
    <div className="kv-grid">
      {shown.map(([k, v], i) => (
        <div key={i} style={{ display: 'contents' }}>
          <div className="k">{FINGER_LABEL[normKey(k)] || FINGER_LABEL[k] || k}</div>
          <FormulaCell k={k} text={fingerValue(k, v)} />
        </div>
      ))}
    </div>
  );
}

/* ---------------- 対戦履歴 ---------------- */

/// 対戦履歴の日時 ("01 Aug 2026 00:10:59") を日本語表記にする。
function readAt(at: string): string {
  const t = Date.parse(at + ' GMT-0600');   // GGS の時刻は MDT/MST 表記
  if (Number.isNaN(t)) return at;
  return new Date(t).toLocaleString('ja-JP', {
    year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit',
  });
}

function History({ ctx, name }: { ctx: GgsCtx; name: string }) {
  const rows = ctx.snap.history[name];
  if (!rows) return <div className="empty">取得中…</div>;
  if (!rows.length) return <div className="empty">対戦履歴がありません。</div>;
  const nameCell = (who: string, rating: string) => (
    <td>
      <span className={who === name ? 'strong' : ''}>{who}</span>{' '}
      <span className="muted small">{rating}</span>
    </td>
  );
  return (
    <table className="history">
      <thead>
        <tr>
          <th>日時</th><th>黒</th><th>白</th>
          <th className="right">石差</th><th>形式</th><th></th>
        </tr>
      </thead>
      <tbody>
        {rows.slice(0, 100).map((r) => {
          const sc = parseFloat(r.score);
          const mine = r.black === name ? sc : -sc;
          return (
            <tr key={r.id}>
              <td className="muted nowrap">{readAt(r.at)}</td>
              {nameCell(r.black, r.black_rating)}
              {nameCell(r.white, r.white_rating)}
              <td className="right">
                <span className={'score ' + (mine > 0 ? 'win' : mine < 0 ? 'loss' : 'draw')}>
                  {r.score}
                </span>
              </td>
              <td className="muted nowrap">{gtypeLabel(r.gtype)}</td>
              <td className="right">
                <button className="btn small"
                        onClick={() => ctx.fetchKifu(r.id, `${r.black} 対 ${r.white}`)}>
                  棋譜
                </button>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
