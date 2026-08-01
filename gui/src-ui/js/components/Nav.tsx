// 左メニュー。ローカルの対局・検討と、GGS の各画面を切り替える。
// GGS 側は統合の段階 3 以降で中身が入る。
export type View =
  | 'play' | 'study'
  | 'ggs-play' | 'ggs-lobby' | 'ggs-users' | 'ggs-chat' | 'ggs-standby' | 'ggs-console';

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
];

export interface NavProps {
  view: View;
  onView: (v: View) => void;
  /** GGS に繋がっているか。未接続なら淡く出す。 */
  online: boolean;
  onSettings: () => void;
}

export function Nav({ view, onView, online, onSettings }: NavProps) {
  const item = ([v, label]: [View, string], ggs = false) => (
    <button key={v}
            className={'nav-item' + (ggs ? ' ggs' : '') + (view === v ? ' active' : '')}
            onClick={() => onView(v)}>
      {label}
    </button>
  );

  return (
    <nav id="nav">
      <div className="nav-brand">KUROOBI</div>
      {LOCAL.map((x) => item(x))}
      <div className="nav-sep">
        <span className={'conn-dot' + (online ? '' : ' off')} />
        GGS
      </div>
      {GGS.map((x) => item(x, true))}
      <span className="nav-spacer" />
      <button className="btn icon ghost" title="設定" aria-label="設定" onClick={onSettings}>
        <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor"
             strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9c.14.36.4.66.73.86.3.18.65.28 1 .28H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      </button>
    </nav>
  );
}
