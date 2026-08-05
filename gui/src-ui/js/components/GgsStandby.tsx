// 待機モード。対局終了 → 間隔待ち → 自動申し込みの繰り返し。
// サーバー側の申し込み条件は今の値を見せるだけで、編集は「GGS の設定」。
import { useEffect, useState } from 'react';
import { ggsApi } from '../api';
import { parseFormula } from '../ggs';
import type { GgsCtx } from './GgsView';
import { FormulaTree } from './GgsUsers';
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
        <div className="sec">
          <div className="sec-head">
            <h2>連戦の待機</h2>
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
            相手を指定しないときは自分からは申し込まず、届いた申し込みを
            受けるだけになります。
          </p>
        </div>

        <AformCard ctx={ctx} />
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

/* ---------------- 申し込みの条件 (サーバー側) ---------------- */

/// 条件そのものは「GGS の設定」で組む。ここでは今の値を見せて、そちらへ送る。
/// 同じ設定を 2 か所で編集できるようにすると、どちらが本物か分からなくなる。
function AformCard({ ctx }: { ctx: GgsCtx }) {
  const { snap } = ctx;
  useEffect(() => {
    if (snap.login) ggsApi.finger(snap.login).catch(() => {});
  }, [snap.login]);
  const form = (key: 'accept' | 'decline'): string =>
    (snap.fingers[snap.login]?.fields
      .find(([k]) => k.replace(/\s+/g, '').replace(/\(.*\)/, '') === key)?.[1] ?? '')
      .replace(/^\s*:\s*/, '').trim();

  const row = (label: string, src: string) => (
    <div className="kv-row">
      <div className="k">{label}</div>
      <div className="v">
        {src ? <FormulaTree node={parseFormula(src)} top /> : '指定なし'}
      </div>
    </div>
  );

  return (
    <div className="sec">
      <div className="sec-head"><h2>申し込みの条件 (サーバー側)</h2></div>
      <p className="hint">
        アプリを閉じてもサーバー側で有効な条件です。待機モードの保険になります。
      </p>
      <div className="kv-grid">
        {row('自動で受ける条件', form('accept'))}
        {row('自動で断る条件', form('decline'))}
      </div>
      <div className="row actions">
        <span className="spacer" />
        <button className="btn fix" onClick={() => ctx.showView('ggs-engine')}>
          条件を変える
        </button>
      </div>
    </div>
  );
}
