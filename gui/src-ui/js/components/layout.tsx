import React from 'react';
import { BoardDefs } from './board';
import { Segmented } from './primitives';
import { IconButton } from './Icons';
// アセットを唯一の出所にする (assets.d.ts の方針)。画面用に写した複製と
// ファイルが食い違う事故を防ぐため、<img src> ではなく中身を読む
import icon from '../../assets/icon.svg?raw';

/* KUROOBI layout
 * 画面は 左メニュー（ggs.tsx の Nav）＋右に縦 3 段
 * （Toolbar / 本体 / StatusBar）。段の高さは固定。中身が増えても外形は動かない。
 * TitleBar は置かない — 行き先は左メニューが持つので、上にもタブを出すと
 * ナビが 2 か所になる。信号機とドラッグ領域は Nav の上端が受ける
 * （Tauri。titleBarStyle: "Overlay" が前提）。
 *
 * 「数枚から 1 枚を選ぶ」列は Segmented 1 つに寄せた（Dock も BottomPanel も）。
 * 選択中は --card で浮かせる — 塗り（--accent-dim）は左メニューの現在地に
 * 取ってあるので、青地を画面に 2 か所出すと「いまどこにいるか」が薄まる。
 */

/* Toasts が position:absolute で右下に付くので、ここが基準になる。
 * relative を外すと報せが画面の左上へ飛ぶ。
 *
 * 畳の <pattern> もここが描く。id の重複を避けるためにドキュメントに 1 つだけ
 * 必要で、AppFrame は必ず 1 つしかない。置き忘れると url(#kb-tatami) が解決せず、
 * エラーも出ずに畳が無地の緑になる — 静かな壊れ方なので忘れようがない形にする。 */
export function AppFrame({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ position: 'relative', height: '100%', display: 'flex', background: 'var(--bg)' }}>
      <BoardDefs />
      {children}
    </div>
  );
}

/* 左メニューの右。Toolbar → 本体 → StatusBar を縦に積む */
export function Main({ children }: { children: React.ReactNode }) {
  return <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0 }}>{children}</div>;
}

/* 盤の上の帯。そのモードの「操作」と、対局の前提を決める最小限だけを置く。
 * 数値は StatusBar へ。
 *
 * aux は 940px 以下で消える（base.css の k-toolbar-aux）。消えて困るものを
 * 入れないこと — 担当のように狭い窓でも変えられなければならないものは
 * children 側に置く。型で分けてあるのは、画面ごとに手でクラスを付けると
 * 必ず付け忘れるから。
 * dock は畳む段（1120px 以下）でだけ出る Dock の開閉。Dock は本体の右に
 * あるものなので、入口も上の帯に置く（BottomPanel の onClose と対称）。
 */
export function Toolbar({ children, aux, dock }: {
  children: React.ReactNode;
  aux?: React.ReactNode;
  dock?: { open: boolean; onToggle: () => void; label?: string };
}) {
  return (
    /* 窓を掴める帯でもある。左メニューの上端だけだと掴める幅が 208px しか
       なく、しかもロゴの上では掴めない (属性を持つ要素そのものでしか効かない)。
       ここは帯の地の部分だけが掴める — 子のボタンは属性を持たないので、
       押すつもりが窓を動かすことはない。 */
    <div data-tauri-drag-region style={{
      height: 'var(--h-bar)', flex: 'none', borderBottom: '1px solid var(--border-weak)',
      display: 'flex', alignItems: 'center', padding: '0 var(--sp-4)', gap: 'var(--sp-2)',
    }}>
      {children}
      {/* 押し出しはこれが持つ。畳む段で消える要素（k-toolbar-aux）に
          marginLeft:auto を持たせると、消えた瞬間に後ろのものが左へ飛ぶ */}
      <span data-tauri-drag-region style={{ flex: 1 }} />
      {/* display は .k-toolbar-aux が持つ（940px で消す） */}
      {aux && <span className="k-toolbar-aux" style={{ alignItems: 'center', gap: 'var(--sp-3)' }}>{aux}</span>}
      {dock && (
        <span className="k-dock-toggle">
          {/* panel = 縦に二分された矩形。ggs-console（左メニューのコンソール）を
              使い回すと、同じ絵が別の意味で 2 か所に出る */}
          <IconButton name="panel" label={dock.label ?? (dock.open ? 'パネルを閉じる' : 'パネルを開く')}
                      onClick={dock.onToggle} />
        </span>
      )}
    </div>
  );
}

export function Dock({ tabs, active, onTab, children, open, scroll = true }: {
  tabs: string[]; active: string; onTab?: (t: string) => void; children: React.ReactNode;
  /** 畳む段（1120px 以下）で出し入れする。広い窓では無視される。
   *  true にする操作は Toolbar の dock={{...}} が持つ。 */
  open?: boolean;
  /** 中身を丸ごとスクロールさせる。**自前で見出しを固定したい中身は false**
   *  (棋譜の表は列の見出しと操作を残したまま行だけ流したい)。 */
  scroll?: boolean;
}) {
  return (
    // width と display は base.css の .k-dock が持つ（1120px で消すので、
    // インラインに書くと media query が届かない）。k-open で重ねて出る。
    <aside className={'k-dock' + (open ? ' k-open' : '')} style={{
      flex: 'none', background: 'var(--panel)',
      borderLeft: '1px solid var(--border)', flexDirection: 'column', minHeight: 0,
    }}>
      <div style={{ padding: 'var(--sp-2)', flex: 'none' }}>
        <Segmented fill value={active} onChange={onTab}
                   options={tabs.map(t => ({ value: t, label: t }))} />
      </div>
      <div className={scroll ? 'k-scroll' : undefined} style={{
        flex: 1, minHeight: 0, display: scroll ? undefined : 'flex', flexDirection: 'column',
      }}>{children}</div>
    </aside>
  );
}

/* 節。丸四角の箱は使わず、見出し＋1px 罫だけで区切る。
 * 中身は無くてもよい — 節そのものが「ここから先は別の話」の印なので、
 * 器がスクロールを持つ場合 (通信ログなど) は見出しだけを置く */
export function Section({ title, aside, children }: { title: string; aside?: React.ReactNode; children?: React.ReactNode }) {
  return (
    <section style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)', padding: '0 var(--sp-3) var(--sp-4)' }}>
      {/* 操作が乗るときは帯を高くする。--h-head (20px) のままだと押せるものが
          罫にめり込み、当たりも文字ぶんしか無くなる (実際に指摘が出た) */}
      <h3 style={{
        margin: 0, minHeight: aside ? 'var(--h-field)' : 'var(--h-head)',
        display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        paddingBottom: aside ? 'var(--sp-1)' : 0,
        fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)',
        borderBottom: '1px solid var(--border)',
      }}>{title}{aside && <span style={{
        marginLeft: 'auto', letterSpacing: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
      }}>{aside}</span>}</h3>
      {children}
    </section>
  );
}

/* 右端の内容はモードごとに変える。GGS の対局中だけ チャット / コンソール を出す */
export function StatusBar({ left, right }: { left?: React.ReactNode; right?: React.ReactNode }) {
  return (
    <div style={{
      height: 'var(--h-status)', flex: 'none', background: 'var(--bg)', borderTop: '1px solid var(--border)',
      display: 'flex', alignItems: 'center', gap: 'var(--sp-4)', padding: '0 var(--sp-4)',
      fontSize: 'var(--fs-6)', color: 'var(--sub)', whiteSpace: 'nowrap',
    }}>
      {left}
      <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>{right}</span>
    </div>
  );
}

export function StatusStat({ label, value, unit }: { label?: string; value: React.ReactNode; unit?: string }) {
  return (
    <span>{label && label + ' '}<b style={{ color: 'var(--text)', fontWeight: 600 }}>{value}</b>{unit && ' ' + unit}</span>
  );
}

/* GGS だけが持つ下部パネル。チャットとコンソールが 1 枚を共有する。
 *
 * 高さは固定（180〜420 の間で決め打ち）。掴んで変えられる見た目を出していたが、
 * 掴めるのに動かないのは無いより悪いので摘みは落とした。現行にも高さ変更は無い。
 * 入れるときは onResize を足し、上端に k-grip の摘みを戻す。
 *
 * タブは未読バッジを持つので Segmented ではないが、選択中の表現は揃える（--card）。
 */
export function BottomPanel({ tabs, active, onTab, onClose, height = 240, children }: {
  tabs: { id: string; label: string; unread?: number }[];
  active: string; onTab?: (id: string) => void; onClose?: () => void;
  height?: number; children: React.ReactNode;
}) {
  return (
    <div style={{
      flex: 'none', height: Math.max(180, Math.min(420, height)),
      background: 'var(--panel)', borderTop: '1px solid var(--border)',
      display: 'flex', flexDirection: 'column',
    }}>
      <div style={{
        height: 36, flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
        padding: '0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
      }}>
        {tabs.map(t => {
          const on = active === t.id;
          return (
            <button key={t.id} type="button" onClick={() => onTab?.(t.id)}
              aria-pressed={on} className={'k-press' + (on ? ' k-on' : '')}
              style={{
                height: 'var(--h-chip)', padding: '0 10px', border: 0, borderRadius: 'var(--r-1)', fontSize: 'var(--fs-6)',
                background: on ? 'var(--card)' : 'transparent',
                color: on ? 'var(--text)' : 'var(--sub)', fontWeight: on ? 600 : 400,
                display: 'flex', alignItems: 'center', gap: 6,
              }}>
              {t.label}
              {t.unread ? <span style={{ background: 'var(--bad)', color: 'var(--on-bad)', borderRadius: 'var(--r-pill)', padding: '0 5px', fontSize: 'var(--fs-7)' }}>{t.unread}</span> : null}
            </button>
          );
        })}
        {onClose && (
          <span style={{ marginLeft: 'auto' }}>
            <IconButton name="close" label="パネルを閉じる" onClick={onClose} size={14} />
          </span>
        )}
      </div>
      <div style={{ flex: 1, display: 'flex', minHeight: 0 }}>{children}</div>
    </div>
  );
}


/* 浮くもの。角丸 --r-4 と影を持つのは Modal / Toast / 盤の外枠だけ
 * (Popover は使い所が無かったので削除した — 規則 70) */
export function Modal({ title, body, actions, width = 340 }: {
  title: string; body?: React.ReactNode; actions?: React.ReactNode;
  /** 既定は 340px。棋譜ビューアのように中身の要る覆いは広げる (規則 71) */
  width?: number;
}) {
  return (
    <div role="dialog" aria-modal style={{
      width, borderRadius: 'var(--r-4)', background: 'var(--card)',
      border: '1px solid var(--border)', boxShadow: 'var(--sh-2)',
      padding: 'var(--sp-5)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-4)',
    }}>
      <div style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{title}</div>
      {body && <div style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)', lineHeight: 1.7 }}>{body}</div>}
      {actions && <div style={{ display: 'flex', gap: 'var(--sp-2)', justifyContent: 'flex-end' }}>{actions}</div>}
    </div>
  );
}

/* 押せない盤を出さないための空状態。GGS 未対局時など */
export function EmptyState({ title, body, actions }: { title: string; body?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <div style={{ flex: 1, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)' }}>
      <div style={{ maxWidth: 420, textAlign: 'center', display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 'var(--sp-5)' }}>
        <span aria-hidden style={{ width: 64, height: 64, borderRadius: 14, overflow: 'hidden', opacity: .5, display: 'block' }}
              dangerouslySetInnerHTML={{ __html: icon }} />
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
          <div style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{title}</div>
          {body && <div style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)', lineHeight: 1.8 }}>{body}</div>}
        </div>
        {actions && <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>{actions}</div>}
      </div>
    </div>
  );
}

/* 浮くものを画面の真ん中に置く暗幕。Modal はこれに入れる。
 *
 * 押せない場所を作るのが役目なので、暗幕そのものを押したら閉じる。
 * Esc でも閉じる — 開いたら閉じ方が要る、というだけの話だが、
 * ブラウザの confirm() と違って自分で書かないと付いてこない。 */
export function Overlay({ onClose, children }: { onClose?: () => void; children: React.ReactNode }) {
  React.useEffect(() => {
    if (!onClose) return;
    const on = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', on);
    return () => window.removeEventListener('keydown', on);
  }, [onClose]);
  return (
    <div onClick={onClose} style={{
      position: 'absolute', inset: 0, zIndex: 30,
      background: 'var(--scrim)', display: 'grid', placeItems: 'center',
      padding: 'var(--sp-5)',
    }}>
      {/* 中身を押したときに閉じない */}
      <div onClick={(e) => e.stopPropagation()}>{children}</div>
    </div>
  );
}
