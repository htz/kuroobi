// ロビー。進行中の対局 (観戦できる)・対局の申し込み・申し込みフォーム・
// 中断対局。
import { useState } from 'react';
import { ggsApi } from '../api';
import { gtypeLabel } from '../ggs';
import type { GgsCtx } from './GgsView';

const ASK_TYPES: [string, string][] = [
  ['s8r16', '同期・ランダム16手 (推奨)'],
  ['s8r18', '同期・ランダム18手'],
  ['s8r20', '同期・ランダム20手'],
  ['s8', '同期・通常開局'],
  ['8', '通常 (1局)'],
  ['8r16', '通常・ランダム16手'],
];
const ASK_TIMES: [string, string][] = [
  ['00:05:00', '5 分'],
  ['00:10:00', '10 分'],
  ['00:15:00', '15 分'],
  ['00:20:00', '20 分'],
  ['00:30:00', '30 分'],
];

/** 相手を選ぶプルダウン。接続中のユーザーで埋める。 */
export function OpponentSelect({ ctx, value, onChange }: {
  ctx: GgsCtx; value: string; onChange: (v: string) => void;
}) {
  const names = ctx.snap.users
    .filter((u) => u.name !== ctx.snap.login)
    .map((u) => u.name);
  return (
    <div className="selwrap">
      <select value={names.includes(value) ? value : ''}
              onChange={(e) => onChange(e.target.value)}>
        <option value="">指定しない (誰でも)</option>
        {names.map((n) => <option key={n} value={n}>{n}</option>)}
      </select>
    </div>
  );
}

export function GgsLobby({ ctx, initialOpp = '' }: { ctx: GgsCtx; initialOpp?: string }) {
  const { snap } = ctx;
  // プレイヤー詳細の「対局を申し込む」から来たときは相手が入っている
  const [opp, setOpp] = useState(initialOpp);
  const [gtype, setGtype] = useState('s8r16');
  const [time, setTime] = useState('00:15:00');

  const games = snap.ongoing.filter((o) => !o.mine);

  return (
    <div className="ggs-cols">
      <div className="col-main">
        <div className="card">
          <div className="card-head">
            <h2>対局中</h2>
            <span className="muted">{games.length ? `${games.length} 局` : ''}</span>
            <button className="btn small" onClick={() => void ggsApi.listMatches()}>更新</button>
          </div>
          <div className="scroll" style={{ maxHeight: 340 }}>
            {!games.length && <div className="empty">進行中の対局はありません。</div>}
            {games.map((g) => {
              const [n1, n2] = g.names;
              const [r1, r2] = g.ratings;
              return (
                <div key={g.id} className="offer">
                  <div className="info">
                    <div className="title">
                      <span>{n1 || '?'}</span>
                      {r1 && <span className="prate">{r1}</span>}
                      <span className="vs">対</span>
                      <span>{n2 || '?'}</span>
                      {r2 && <span className="prate">{r2}</span>}
                    </div>
                    <div className="sub">{gtypeLabel(g.gtype)}</div>
                  </div>
                  <button className={'btn small' + (g.watching ? ' danger' : ' primary')}
                          onClick={() => {
                            const on = !g.watching;
                            void ggsApi.watch(g.id, on);
                            // 観戦を始めたら盤が見たいはずなので対局画面へ移る
                            if (on) ctx.showView('ggs-play');
                          }}>
                    {g.watching ? '観戦をやめる' : '観戦'}
                  </button>
                </div>
              );
            })}
          </div>
        </div>

        <div className="card grow">
          <div className="card-head"><h2>対局の申し込み</h2></div>
          <div className="scroll grow">
            {!snap.offers.length && <div className="empty">対局の申し込みはありません。</div>}
            {snap.offers.map((o) => {
              const names = o.names.filter((n) => n !== snap.login);
              return (
                <div key={o.id} className={'offer' + (o.incoming ? ' incoming' : '')}>
                  <div className="info">
                    <div className="title">
                      {names.join(' と ') || '?'}
                      {o.incoming && <span className="tag">自分宛</span>}
                    </div>
                    <div className="sub">
                      {`${gtypeLabel(o.gtype)} · ${o.time || '?'}${o.rated ? ' · レート戦' : ''}`}
                    </div>
                  </div>
                  {names.length === 1 && (
                    <button className="btn small"
                            onClick={() => ctx.showUser(names[0])}>情報</button>
                  )}
                  {o.incoming && (
                    <>
                      <button className="btn small primary"
                              onClick={() => void ggsApi.accept(o.id)}>受ける</button>
                      <button className="btn small danger"
                              onClick={() => void ggsApi.decline(o.id)}>断る</button>
                    </>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </div>

      <aside className="col-side">
        <div className="card">
          <div className="card-head"><h2>対局を申し込む</h2></div>
          <label className="field">相手</label>
          <OpponentSelect ctx={ctx} value={opp} onChange={setOpp} />
          <label className="field">形式</label>
          <div className="selwrap">
            <select value={gtype} onChange={(e) => setGtype(e.target.value)}>
              {ASK_TYPES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
            </select>
          </div>
          <label className="field">持ち時間</label>
          <div className="selwrap">
            <select value={time} onChange={(e) => setTime(e.target.value)}>
              {ASK_TIMES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
            </select>
          </div>
          <button className="btn primary" disabled={!opp}
                  onClick={() => void ggsApi.ask(gtype, time, opp)}>申し込む</button>
          <p className="hint">
            同期対局は同じ開局を先後入れ替えて 2 局同時に行い、結果は合計で
            判定します。レートは「ランダム開局」に反映されます。
          </p>
        </div>
        <div className="card">
          <div className="card-head">
            <h2>中断対局</h2>
            <button className="btn small" onClick={() => void ggsApi.listStored()}>更新</button>
          </div>
          <div className="scroll" style={{ maxHeight: 150 }}>
            {!snap.stored.length && <div className="empty">中断対局はありません。</div>}
            {snap.stored.map((x) => (
              <div key={x.id} className="offer">
                <div className="info">
                  <div className="title">{x.opp || '?'}</div>
                  <div className="sub">{gtypeLabel(x.gtype)}</div>
                </div>
                <button className="btn small primary"
                        onClick={() => void ggsApi.resumeStored(x.id)}>再開</button>
              </div>
            ))}
          </div>
        </div>
      </aside>
    </div>
  );
}
