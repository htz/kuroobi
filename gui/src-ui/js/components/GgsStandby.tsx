// 待機モード。対局終了 → 間隔待ち → 自動申し込みの繰り返しと、
// サーバー側の自動受諾条件 (aform) の設定。
import { useState } from 'react';
import { ggsApi } from '../api';
import type { GgsCtx } from './GgsView';
import { OpponentSelect } from './GgsLobby';

const SB_TYPES: [string, string][] = [
  ['s8r16', '同期・ランダム16手 (推奨)'],
  ['s8r18', '同期・ランダム18手'],
  ['s8', '同期・通常開局'],
  ['8', '通常 (1局)'],
];
const SB_TIMES: [string, string][] = [
  ['00:05:00', '5 分'],
  ['00:10:00', '10 分'],
  ['00:15:00', '15 分'],
  ['00:20:00', '20 分'],
];

export function GgsStandby({ ctx }: { ctx: GgsCtx }) {
  const { snap } = ctx;
  const sb = snap.standby;
  const st = snap.standby_stats;
  const [opp, setOpp] = useState(sb.opponent);
  const [gtype, setGtype] = useState(sb.gtype || 's8r16');
  const [time, setTime] = useState(sb.time || '00:15:00');
  const [maxGames, setMaxGames] = useState(sb.max_games);
  const [interval, setIntervalSecs] = useState(sb.interval_secs || 20);
  const [autoAccept, setAutoAccept] = useState(sb.auto_accept);

  const toggle = () => {
    void ggsApi.setStandby({
      enabled: !sb.enabled,
      auto_accept: autoAccept,
      opponent: opp.trim(),
      gtype,
      time,
      max_games: maxGames,
      interval_secs: interval,
    });
  };

  return (
    <div className="ggs-cols">
      <div className="col-main">
        <div className="card">
          <div className="card-head">
            <h2>放置で連戦する</h2>
            <span className="spacer" />
            <div className="stat-strip">
              <span className={'run-state ' + (sb.enabled ? 'on' : 'off')}>
                {sb.enabled ? (snap.matches.length ? '対局中' : '申し込み待ち') : '停止中'}
              </span>
              <Stat v={String(st.games)} label="局" />
              <Stat v={String(st.wins)} label="勝" color="var(--ok)" />
              <Stat v={String(st.losses)} label="敗" color="var(--bad)" />
              <Stat v={String(st.draws)} label="分" />
              <Stat v={`${st.diff_sum > 0 ? '+' : ''}${st.diff_sum}`} label="石差" />
            </div>
          </div>
          <div className="row">
            <div>
              <label className="field">相手</label>
              <OpponentSelect ctx={ctx} value={opp} onChange={setOpp} />
            </div>
            <div>
              <label className="field">形式</label>
              <div className="selwrap">
                <select value={gtype} onChange={(e) => setGtype(e.target.value)}>
                  {SB_TYPES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
                </select>
              </div>
            </div>
            <div>
              <label className="field">持ち時間</label>
              <div className="selwrap">
                <select value={time} onChange={(e) => setTime(e.target.value)}>
                  {SB_TIMES.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
                </select>
              </div>
            </div>
          </div>
          <div className="row">
            <div>
              <label className="field">最大対局数 (0 = 無制限)</label>
              <input type="number" min={0} value={maxGames}
                     onChange={(e) => setMaxGames(+e.target.value)} />
            </div>
            <div>
              <label className="field">対局の間隔 (秒)</label>
              <input type="number" min={5} value={interval}
                     onChange={(e) => setIntervalSecs(+e.target.value)} />
            </div>
          </div>
          <label className="check">
            <input type="checkbox" checked={autoAccept}
                   onChange={(e) => setAutoAccept(e.target.checked)} />
            届いた申し込みを自動で受ける
          </label>
          <button className={'btn' + (sb.enabled ? '' : ' primary')} onClick={toggle}>
            {sb.enabled ? '待機モードを停止' : '待機モードを開始'}
          </button>
          <p className="hint">
            対局終了 → 間隔待ち → 自動申し込みを繰り返します。
            切断時は自動で再接続し、中断対局も自動再開します。
          </p>
        </div>

        <AformCard />
      </div>
    </div>
  );
}

function Stat({ v, label, color }: { v: string; label: string; color?: string }) {
  return (
    <span className="stat-item">
      <span className="stat-num" style={color ? { color } : undefined}>{v}</span>
      <span className="muted">{label}</span>
    </span>
  );
}

/* ---------------- 自動受諾の条件 (サーバー側) ---------------- */

function AformCard() {
  const [rated, setRated] = useState(true);
  const [size8, setSize8] = useState(true);
  const [sync, setSync] = useState(false);
  const [noSaved, setNoSaved] = useState(true);
  const [minRate, setMinRate] = useState(false);
  const [minRateV, setMinRateV] = useState(2000);
  const [minTime, setMinTime] = useState(false);
  const [minTimeV, setMinTimeV] = useState(600);

  const build = (): string => {
    const parts = [];
    if (rated) parts.push('rated');
    if (size8) parts.push('size=8');
    if (sync) parts.push('synchro');
    if (noSaved) parts.push('!saved');
    if (minRate) parts.push(`or>=${minRateV}`);
    if (minTime) parts.push(`mt1>=${minTimeV}`);
    return parts.join('&');
  };

  const check = (label: React.ReactNode, on: boolean, set: (b: boolean) => void) => (
    <label className="check">
      <input type="checkbox" checked={on} onChange={(e) => set(e.target.checked)} />
      {label}
    </label>
  );

  return (
    <div className="card">
      <div className="card-head"><h2>自動で受ける条件 (サーバー側)</h2></div>
      <p className="hint">
        アプリを閉じてもサーバー側で有効な条件です。待機モードの保険になります。
      </p>
      <div className="checks">
        {check('レート戦のみ', rated, setRated)}
        {check('8x8 のみ', size8, setSize8)}
        {check('同期対局のみ', sync, setSync)}
        {check('中断対局は除く', noSaved, setNoSaved)}
        <label className="check">
          <input type="checkbox" checked={minRate}
                 onChange={(e) => setMinRate(e.target.checked)} />
          相手レート
          <input type="number" className="inline-num" value={minRateV}
                 onChange={(e) => setMinRateV(+e.target.value)} />
          以上
        </label>
        <label className="check">
          <input type="checkbox" checked={minTime}
                 onChange={(e) => setMinTime(e.target.checked)} />
          持ち時間
          <select className="inline-num" value={minTimeV}
                  onChange={(e) => setMinTimeV(+e.target.value)}>
            <option value={300}>5 分</option>
            <option value={600}>10 分</option>
            <option value={900}>15 分</option>
          </select>
          以上
        </label>
      </div>
      <div className="row actions">
        <button className="btn fix"
                onClick={() => void ggsApi.setFormula('aform', '')}>解除</button>
        <span className="spacer" />
        <button className="btn primary fix"
                onClick={() => void ggsApi.setFormula('aform', build())}>条件を設定</button>
      </div>
    </div>
  );
}
