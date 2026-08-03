// 左メニュー。ローカルの対局・検討と、GGS の各画面を切り替える。
// 下部に「いま何が CPU を使っているか」を常時出す (対局と検討・学習が
// CPU を食い合う構成なので、何が走っているか見えないと切り分けられない)。
import type { ActivityView } from '../api';
import { Icon } from './Icons';
// ロゴは SVG ファイルをそのまま読む (色は currentColor で地に追従する)。
import logo from '../../assets/kuroobi.svg?raw';

export type View =
  | 'play' | 'study'
  | 'ggs-play' | 'ggs-lobby' | 'ggs-users' | 'ggs-chat' | 'ggs-standby' | 'ggs-console'
  | 'ggs-engine';

const LOCAL: [View, string][] = [
  ['play', '対局'],
  ['study', '検討'],
];
const GGS: [View, string][] = [
  ['ggs-play', '対局・観戦'],
  ['ggs-lobby', 'ロビー'],
  ['ggs-users', 'プレイヤー'],
  ['ggs-chat', 'チャット'],
  ['ggs-standby', '待機モード'],
  ['ggs-console', 'コンソール'],
  ['ggs-engine', 'GGS の設定'],
];

export interface NavProps {
  view: View;
  onView: (v: View) => void;
  /** GGS に繋がっているか。未接続なら淡く出す。 */
  online: boolean;
  /** 画面ごとの件数バッジ (申し込み・対局・チャット未読)。 */
  badges?: Partial<Record<View, number>>;
  /** CPU の稼働状況 (1 秒ごとの取得値)。 */
  cpu?: ActivityView | null;
  onSettings: () => void;
}

export function Nav({ view, onView, online, badges, cpu, onSettings }: NavProps) {
  const item = ([v, label]: [View, string]) => {
    const n = badges?.[v] ?? 0;
    return (
      <button key={v}
              className={'nav-item' + (view === v ? ' active' : '')}
              onClick={() => onView(v)}>
        <Icon name={v === 'ggs-engine' ? 'gear' : v} size={16} />
        {label}
        {n > 0 && <span className="badge-n">{n}</span>}
      </button>
    );
  };

  return (
    <nav id="nav">
      {/* 自前のアセットなので挿入して構わない。SVG のまま入れることで
          currentColor が効き、地の明暗に合わせて色が変わる */}
      <div className="nav-brand" aria-label="Kuroobi"
           dangerouslySetInnerHTML={{ __html: logo }} />
      {LOCAL.map((x) => item(x))}
      <div className="nav-sep">
        <span className={'conn-dot' + (online ? '' : ' off')} />
        GGS
      </div>
      {/* 各画面はログイン後にだけ出す (未接続では中身が無いため)。
          未接続のあいだは代わりに「ログイン」を置く */}
      {online ? (
        GGS.map((x) => item(x))
      ) : (
        <button className={'nav-item' + (view.startsWith('ggs') ? ' active' : '')}
                onClick={() => onView('ggs-play')}>
          <Icon name="login" size={16} />
          ログイン
        </button>
      )}
      <span className="nav-spacer" />
      <CpuStatus cpu={cpu} />
      <button className="btn icon ghost" title="設定" aria-label="設定" onClick={onSettings}>
        <Icon name="gear" />
      </button>
    </nav>
  );
}

const gb = (bytes: number) => bytes / 1024 ** 3;

/// 資源の使用状況。CPU とメモリをバーで出し、その下に動いている機能を並べる。
/// バーの満杯は CPU が全コア、メモリが積んでいる総量。
function CpuStatus({ cpu }: { cpu?: ActivityView | null }) {
  if (!cpu) return null;
  const rows: string[] = [];
  if (cpu.ggs_match) {
    rows.push(`GGS対局${cpu.ggs_thinking ? '·思考中' : ''} (${cpu.ggs_threads}スレ)`);
  }
  if (cpu.local) rows.push(`${cpu.local}中 (${cpu.local_threads}スレ)`);
  if (cpu.learn) {
    rows.push(`学習 ${cpu.learn[0]}/${cpu.learn[1]}${cpu.learn_paused ? ' 譲り中' : ''}`);
  }
  // 常に実数を出す。「待機」と書くと 0% に見えるが、画面が動いている
  // 限り 0 にはならない。低い値が 0% に丸まらないよう、10% 未満は小数第 1 位まで
  const pct = cpu.cpu < 10 ? cpu.cpu.toFixed(1) : String(Math.round(cpu.cpu));
  // 全コアに対する割合。1 コア未満の待機時でもバーが見えるよう下限を置く
  // 0 でない限りバーが見えるように下限を置く (64GB 中 1GB のような比率でも
  // 「使っている」ことは伝えたい)
  const width = (r: number) => `${Math.min(100, Math.max(r > 0 ? 2 : 0, r))}%`;
  const cpuFill = (cpu.cpu / Math.max(1, cpu.cores * 100)) * 100;
  // 色は「余裕がある / 使い込んでいる / 使い切っている」の 3 段
  const level = (r: number) => (r >= 80 ? ' hot' : r >= 40 ? ' busy' : '');
  const memRatio = cpu.mem_total ? (cpu.mem / cpu.mem_total) * 100 : 0;

  return (
    <div className="cpu-status">
      <div className="meter">
        <div className="meter-head">
          <span className="label"><Icon name="cpu" size={13} />CPU</span>
          <span className="val">{pct}<span className="unit">%</span></span>
        </div>
        <div className="bar"><i className={'fill' + level(cpuFill)}
                                style={{ width: width(cpuFill) }} /></div>
        <div className="sub">{cpu.cores} コア中 {(cpu.cpu / 100).toFixed(1)} 相当</div>
      </div>
      {cpu.mem_total > 0 && (
        <div className="meter">
          <div className="meter-head">
            <span className="label"><Icon name="memory" size={13} />メモリ</span>
            <span className="val">{gb(cpu.mem).toFixed(1)}<span className="unit">GB</span></span>
          </div>
          <div className="bar"><i className={'fill' + level(memRatio)}
                                  style={{ width: width(memRatio) }} /></div>
          <div className="sub">全 {gb(cpu.mem_total).toFixed(0)} GB</div>
        </div>
      )}
      {rows.map((r) => <div key={r} className="cpu-row">{r}</div>)}
    </div>
  );
}
