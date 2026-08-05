// GGS の対局・観戦。左に手合いの一覧 (同期対局は 2 局で 1 組) と対局結果、
// 右に選んだ手合いの盤。盤はローカル対局と同じ Board 部品で描く。
import { useState } from 'react';
import { ggsApi } from '../api';
import type { GameResult, GgsSnapshot, MatchView } from '../types';
import { countDiscs, ggsMoveToIndex, gtypeLabel, kifuText, relTime, useClocks } from '../ggs';
import type { ClockSide, ClockView } from '../ggs';
import { Board } from './Board';
import type { GgsCtx } from './GgsView';

export function GgsPlay({ ctx }: { ctx: GgsCtx }) {
  const { snap } = ctx;
  const [sel, setSel] = useState('');
  const clock = useClocks(snap.matches);

  // 手合い (同期対局は 2 局で 1 組) にまとめる
  const groups = new Map<string, MatchView[]>();
  for (const m of snap.matches) {
    const g = groups.get(m.base) ?? [];
    g.push(m);
    groups.set(m.base, g);
  }
  // 自分の対局を先に、次に観戦。選択が消えていたら先頭に移る。
  const keys = [...groups.keys()].sort((a, b) => {
    const mine = (k: string) => (groups.get(k)!.some((m) => m.my_color) ? 0 : 1);
    return mine(a) - mine(b) || a.localeCompare(b);
  });
  const cur = groups.has(sel) ? sel : keys[0] ?? '';
  const pair = groups.get(cur);

  if (!groups.size) {
    return (
      <div className="split-pane no-list">
        <NoMatch online={snap.conn === 'online'} showView={ctx.showView}
                 notice={snap.notice} />
      </div>
    );
  }

  const nameLink = (n: string) => (
    <span className="pname link" onClick={() => ctx.showUser(n)}>{n}</span>
  );

  const mine = pair?.some((x) => x.my_color) ?? false;
  const m0 = pair?.[0];

  return (
    <div className="split-pane">
      <aside className="split-list">
        <div className="split-list-head">
          <h2>手合い</h2>
          <span className="muted">{groups.size ? `${groups.size} 組` : ''}</span>
        </div>
        <div className="scroll grow">
          {keys.map((key) => {
            const g = groups.get(key)!;
            const m = g[0];
            const isMine = g.some((x) => x.my_color);
            const names = isMine
              ? `自分 対 ${m.opp_name || '?'}`
              : (m.players.map((p) => p.name).join(' 対 ') || key);
            const moves = Math.max(...g.map((x) => x.moves.length));
            const thinking = g.some((x) => snap.thinking === x.id);
            // 終局しても一覧からは消さない (盤面から棋譜を取り出せるように)
            const over = g.every((x) => x.over);
            const result = g.map((x) => x.result).find(Boolean) ?? '';
            return (
              <div key={key} className={'thread' + (key === cur ? ' active' : '')}
                   onClick={() => setSel(key)}>
                <div className="thread-top">
                  {/* 種別と状態は別に出す。終局しても自分の対局か観戦かは
                      分かるようにしておきたい */}
                  <span className={'tag ' + (isMine ? 'mine' : 'watch')}>
                    {isMine ? '自分' : '観戦'}
                  </span>
                  <span className={'tag ' + (over ? 'done' : 'live')}>
                    {over ? '終局' : '対局中'}
                  </span>
                  <span className="thread-name">{names}</span>
                  {thinking && <span className="thinking-dot" />}
                  {over && (
                    <button className="btn icon ghost close-x" title="一覧から閉じる"
                            onClick={(e) => {
                              e.stopPropagation();
                              void ggsApi.closeMatch(m.base || m.id);
                            }}>×</button>
                  )}
                </div>
                <div className="thread-last">
                  {`${gtypeLabel(m.gtype)}${g.length === 2 ? ' · 2 局' : ''} · ${moves} 手目`}
                  {over && result && ` · ${result}`}
                </div>
              </div>
            );
          })}
        </div>
        <div className="split-list-foot">
          <div className="sec-head">
            <h2>対局結果</h2>
            <span className="muted">{resultsSummary(snap)}</span>
          </div>
          <div className="scroll"><Results ctx={ctx} /></div>
        </div>
      </aside>

      <div className="split-room">
        <div className="split-room-head">
          <h2 className="match-title">
            {mine
              ? <>自分 対 {m0?.opp_name ? nameLink(m0.opp_name) : '?'}</>
              : m0 && m0.players.length >= 2
                ? <>{nameLink(m0.players[0].name)} 対 {nameLink(m0.players[1].name)}</>
                : cur}
          </h2>
          <span className="muted">{gtypeLabel(m0?.gtype ?? '')}</span>
          <span className="spacer" />
          {pair?.length === 2 && (
            <span className="muted small">同じ開局を先後入れ替えた 2 局。結果は合計で判定</span>
          )}
          {!mine && (
            // 手合いごと (組の全局) をまとめて止める
            <button className="btn small danger"
                    onClick={() => pair?.forEach((m) => void ggsApi.watch(m.id, false))}>
              観戦をやめる
            </button>
          )}
        </div>
        <div className="split-body">
          <div className="ggs-boards">
            {pair?.map((m) => <MatchCard key={m.id} ctx={ctx} m={m} clock={clock} />)}
          </div>
        </div>
      </div>
    </div>
  );
}

function NoMatch({ online, showView, notice }:
    { online: boolean; showView: GgsCtx['showView']; notice: string }) {
  return (
    <div className="no-match">
      <EmptyBoard />
      <div className="no-match-text">
        <div className="no-match-title">
          {online ? '対局はまだありません' : 'ログインしてください'}
        </div>
        <div className="muted">
          {online
            ? (notice || 'ロビーで申し込むか、進行中の対局を観戦できます。')
            : '接続すると、対局と観戦がここに表示されます。'}
        </div>
        {online && (
          <div className="row actions" style={{ marginTop: 18 }}>
            <button className="btn primary" onClick={() => showView('ggs-lobby')}>ロビーへ</button>
            <button className="btn" onClick={() => showView('ggs-standby')}>
              待機モードへ
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

/// 対局がないときに置く、石を並べただけの盤。操作はできない。
///
/// 地は対局盤と同じ畳。1 マスを縁なし半畳 1 枚と見て、市松に藺草の目の向き
/// を変える。viewBox が 1 マス = 1 なので、目の間隔は Board.tsx の 12.5/100
/// と同じ 0.125 になる。
function EmptyBoard() {
  const grain = [];
  for (let f = 0; f < 8; f++) {
    for (let r = 0; r < 8; r++) {
      const vertical = (f + r) % 2 === 1;
      for (let i = 1; i < 8; i++) {
        const d = i * 0.125;
        grain.push(vertical
          ? <line key={`${f}${r}-${i}`} className="eb-grain"
                  x1={f + d} y1={r} x2={f + d} y2={r + 1} />
          : <line key={`${f}${r}-${i}`} className="eb-grain"
                  x1={f} y1={r + d} x2={f + 1} y2={r + d} />);
      }
    }
  }
  return (
    <svg viewBox="0 0 8 8" className="empty-board">
      {Array.from({ length: 64 }, (_, i) => (
        <rect key={i} x={Math.floor(i / 8)} y={i % 8} width={1} height={1} className="eb-cell" />
      ))}
      {grain}
      {([[3, 3, false], [4, 3, true], [3, 4, true], [4, 4, false]] as
          [number, number, boolean][]).map(([f, r, black]) => (
        <circle key={`${f}${r}`} cx={f + 0.5} cy={r + 0.5} r={0.38}
                className={black ? 'eb-black' : 'eb-white'} />
      ))}
    </svg>
  );
}

/// 対局者のレートを詳しく出す。who の生データに偏差 (`2612.4@180.5`) と
/// 対局中フラグが入っているので、そこから拾って添える。
function RateDetail({ snap, name, fallback }: {
  snap: GgsSnapshot; name: string; fallback: string;
}) {
  const u = snap.users.find((x) => x.name === name);
  const m = /(\d+(?:\.\d+)?)@\s*(\d+(?:\.\d+)?)/.exec(u?.raw || '');
  const value = m ? m[1] : (fallback || (u?.rating != null ? u.rating.toFixed(1) : ''));
  if (!value) return null;
  const dev = m ? Math.round(parseFloat(m[2])) : null;
  return (
    <span className="prate">
      {value}
      {dev != null && <span className="dev">±{dev}</span>}
      {/* 偏差が大きいうちはレートが動きやすい。目安を添える。 */}
      {dev != null && dev >= 100 && <span className="prov">暫定</span>}
    </span>
  );
}

function PlayerRow({ ctx, name, rating, color, clock, extra }: {
  ctx: GgsCtx; name: string; rating: string; color: string;
  clock: ClockView; extra?: React.ReactNode;
}) {
  const known = !!name && name !== '?';
  return (
    <div className="player-row">
      <span className={'disc-mark ' + (color === 'black' ? 'b' : 'w')} />
      <span className={'pname' + (known ? ' link' : '')}
            title={known ? `${name} の詳細を見る` : undefined}
            onClick={known ? () => ctx.showUser(name) : undefined}>
        {name || '?'}
      </span>
      <RateDetail snap={ctx.snap} name={name} fallback={rating} />
      {extra}
      <span className={'clock ' + clock.cls}>{clock.text}</span>
    </div>
  );
}

function MatchCard({ ctx, m, clock }: {
  ctx: GgsCtx; m: MatchView; clock: (id: string, side: ClockSide) => ClockView;
}) {
  const { snap } = ctx;
  const observer = !m.my_color;
  const { black, white } = countDiscs(m.cells);
  // 最後に打たれた石に印を付ける (パスは石を置かないので飛ばす)
  const last = [...m.moves].reverse().map(ggsMoveToIndex).find((x) => x !== null) ?? null;

  const myEval = m.last_eval != null && (
    <span className={'eval' + (m.last_from_book ? ' book' : '')}>
      {(m.last_from_book ? '定石 ' : '') +
        (m.last_eval > 0 ? '+' : '') +
        (m.last_eval_exact ? m.last_eval.toFixed(0) : m.last_eval.toFixed(1)) +
        (m.last_eval_exact ? ' 読切' : '')}
    </span>
  );

  // 自分の側もレートを出す。対局中に見たいのは「この 2 人の力量差」。
  const myRate = snap.my_ranks
    .find((r) => r.gtype === (m.gtype.includes('r') ? '8r' : '8'))?.rating;

  return (
    <div className="board-card">
      {observer && m.players.length >= 2 ? (
        <>
          <PlayerRow ctx={ctx} name={m.players[0].name} rating={m.players[0].rating}
                     color={m.players[0].color} clock={clock(m.id, 'p0')} />
          <Board cells={m.cells} last={last} />
          <PlayerRow ctx={ctx} name={m.players[1].name} rating={m.players[1].rating}
                     color={m.players[1].color} clock={clock(m.id, 'p1')} />
        </>
      ) : (
        <>
          <PlayerRow ctx={ctx} name={m.opp_name} rating={m.opp_rating}
                     color={m.my_color === 'black' ? 'white' : 'black'}
                     clock={clock(m.id, 'opp')} />
          <Board cells={m.cells} last={last} />
          <PlayerRow ctx={ctx} name={snap.login}
                     rating={myRate != null ? myRate.toFixed(1) : ''}
                     color={m.my_color} clock={clock(m.id, 'my')} extra={myEval} />
        </>
      )}

      <div className="board-foot">
        <span className="score">
          <span className="disc-mark b" /> {black}
          <span style={{ margin: '0 6px', color: 'var(--sub)' }}>–</span>
          {white} <span className="disc-mark w" />
        </span>
        <span className="muted">{m.moves.length} 手</span>
        {observer && m.watch_eval != null && (
          <span className="eval">
            {`解析 ${m.watch_eval > 0 ? '+' : ''}${m.watch_eval.toFixed(1)}` +
              (m.watch_best ? ` (${m.watch_best})` : '')}
          </span>
        )}
        {snap.thinking === m.id && <span className="ggs-thinking">思考中</span>}
      </div>

      <div className="row">
        <button className="btn small"
                onClick={() => ctx.showKifu(kifuTitle(m, observer),
                                            kifuText(m.ggf, m.moves))}>棋譜</button>
        {/* 観戦をやめるのは手合いごと (上のボタン)。1 局だけ抜けても
            同期対局では片方が残って中途半端になる */}
        {/* 待った (undo) と中止 (abort) は出さない。どちらも相手の承諾が要る
            要求で、GGS の相手はたいていプログラムなので通らない。投了だけは
            自分ひとりで決められるので残す */}
        {!observer && (
          <button className="btn small danger"
                  title="負けを認めて終わる (相手の承諾は要らない。レートが動く)"
                  onClick={() => {
                    if (confirm(`${m.id}: 投了しますか? (負けになります)`)) {
                      void ggsApi.matchCmd(m.id, 'resign');
                    }
                  }}>投了</button>
        )}
      </div>
    </div>
  );
}

/* ---------------- 対局結果 ---------------- */

function resultsSummary(snap: GgsSnapshot): string {
  const rs = snap.results;
  if (!rs.length) return '';
  let w = 0, l = 0, d = 0;
  for (const r of rs) {
    if (r.my_diff === null) continue;
    if (r.my_diff > 0) w++; else if (r.my_diff < 0) l++; else d++;
  }
  return `${rs.length} 局  ${w}勝 ${l}敗 ${d}分`;
}

/// 棋譜を開いたときの題名。**対局 ID は出さない** — サーバが採番しただけの
/// 番号で、人には何も伝えない (盤面で見せてから渡す作りにしたのと同じ理由)。
/// 同期対局は 1 マッチが `.N.0` と `.N.1` の 2 面に分かれるので、何面目かだけ
/// 添える。ID の形は Rust 側の base_id と揃えてある。
function kifuTitle(m: MatchView, observer: boolean): string {
  const names = observer && m.players.length >= 2
    ? m.players.map((p) => p.name).join(' 対 ')
    : `自分 対 ${m.opp_name || '?'}`;
  const parts = m.id.split('.');
  const last = parts[parts.length - 1];
  const nth = parts.length >= 3 && last.length === 1 && !Number.isNaN(+last)
    ? ` — ${+last + 1} 面目`
    : '';
  return names + nth;
}

/// 対局結果から棋譜を開いたときの題名。相手が分からないときも ID は出さない。
function resultTitle(r: GameResult): string {
  return r.opp ? `自分 対 ${r.opp}` : '終わった対局';
}

function Results({ ctx }: { ctx: GgsCtx }) {
  const rs = ctx.snap.results;
  if (!rs.length) return <div className="empty">対局結果はまだありません。</div>;
  return (
    <>
      {rs.slice(0, 60).map((r) => {
        const d0 = r.my_diff ?? 0;
        const cls = d0 > 0 ? 'win' : d0 < 0 ? 'loss' : 'draw';
        return (
          <div key={r.seq + r.id} className="result-item">
            <span className={'diff ' + cls}>
              {r.my_diff === null ? '?' : (d0 > 0 ? '+' : '') + d0}
            </span>
            <span className="meta">{r.opp || r.id}</span>
            <span className="muted" style={{ fontSize: 12 }}>{relTime(r.at)}</span>
            <button className="btn small" disabled={!(r.ggf || r.kifu || r.archive)}
                    onClick={() => (r.ggf || r.kifu)
                      ? ctx.showKifu(resultTitle(r), r.ggf || r.kifu)
                      // 手元に無くても、GGS のアーカイブから取り出せる
                      : ctx.fetchKifu(r.archive, resultTitle(r))}>棋譜</button>
          </div>
        );
      })}
    </>
  );
}
