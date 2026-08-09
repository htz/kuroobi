import React from 'react';
import { BoardDefs } from './board';
import { Button, Segmented } from './primitives';
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
    <div style={{
      position: 'relative', height: '100%', display: 'flex', flexDirection: 'column',
      background: 'var(--bg)',
    }}>
      <BoardDefs />
      {children}
    </div>
  );
}

/* 窓の帯。全幅 28px。**押せるものを 1 つも置かない** — これが層の境目 (規則 75)。
 * 押せるものはすべて画面の側 (Toolbar / Nav / 本体) にいる。
 *
 * 左に信号機ぶんの余白とロゴ、その右に **いま何を見ているか** を受け身の
 * 文字で出す (macOS の題名の役)。Mission Control やスクショでも何をして
 * いたかが分かる。Tauri は title を空にしてこちらで描く。 */
export function WindowBar({ title, sub }: { title: string; sub?: React.ReactNode }) {
  return (
    /* 左右に信号機ぶんの余白を同じだけ取り、題名は**窓の中央**に置く。
       左だけ空けて左寄せにすると、窓の題名ではなく画面の見出しに見える。
       ロゴはここには置かない — 28px の帯に押し込むと窮屈で、Nav の上端の
       ほうが余裕がある。 */
    <div data-tauri-drag-region className="k-drag" style={{
      height: 'var(--h-window)', flex: 'none', background: 'var(--bg)',
      borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center',
      padding: '0 var(--w-signals)', gap: 'var(--sp-2)',
    }}>
      <span data-tauri-drag-region style={{
        margin: '0 auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
        minWidth: 0, fontSize: 'var(--fs-6)',
      }}>
        <span style={{ color: 'var(--text)' }}>{title}</span>
        {sub && (
          <span style={{
            color: 'var(--sub)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}>{sub}</span>
        )}
      </span>
    </div>
  );
}

/* WindowBar と StatusBar に挟まれた中段。Nav / 画面 / Dock を横に並べる。
 * Dock を重ねて出す (k-open) ときの基準もここ — 全幅の帯の下に潜らせない。 */
export function Body({ children }: { children: React.ReactNode }) {
  return <div style={{ position: 'relative', flex: 1, display: 'flex', minHeight: 0 }}>{children}</div>;
}

/* Nav の右。Toolbar → 本体 を縦に積む、**画面そのもの**の列 */
export function Main({ children, inset }: {
  children: React.ReactNode;
  /** 重ねて出した Dock のぶんだけ本体を狭める (畳む段でだけ効く)。 */
  inset?: boolean;
}) {
  return (
    // `k-main` は規則を持っていなかった (base.css に .k-main が無い)。
    // 効かないクラスを付けたままにすると、洗い出しのたびに引っかかる
    <div className={inset ? 'k-dock-inset' : undefined}
         style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0 }}>
      {children}
    </div>
  );
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
 * graph も同じ考え方で、いちばん低い段（620px 以下）でだけ出る評価値グラフの
 * 開閉。畳むと見出し行ごと消えるので、入口は帯に置くしかない。
 */
export function Toolbar({ children, aux, dock, graph }: {
  children: React.ReactNode;
  aux?: React.ReactNode;
  dock?: { open: boolean; onToggle: () => void; label?: string };
  graph?: { open: boolean; onToggle: () => void };
}) {
  return (
    /* 窓を掴める帯でもある。左メニューの上端だけだと掴める幅が 208px しか
       なく、しかもロゴの上では掴めない (属性を持つ要素そのものでしか効かない)。
       ここは帯の地の部分だけが掴める — 子のボタンは属性を持たないので、
       押すつもりが窓を動かすことはない。 */
    /* 画面の帯。Nav の右、本体の上。**その画面だけの操作**を置く。
       信号機の余白もロゴもここには無い — WindowBar が持つ (規則 75)。 */
    <div style={{
      height: 'var(--h-bar)', flex: 'none', borderBottom: '1px solid var(--border-weak)',
      display: 'flex', alignItems: 'center', padding: '0 var(--sp-4)', gap: 'var(--sp-2)',
    }}>
      {children}
      {/* 押し出しはこれが持つ。畳む段で消える要素（k-toolbar-aux）に
          marginLeft:auto を持たせると、消えた瞬間に後ろのものが左へ飛ぶ */}
      <span style={{ flex: 1 }} />
      {/* display は .k-toolbar-aux が持つ（940px で消す） */}
      {aux && <span className="k-toolbar-aux" style={{ alignItems: 'center', gap: 'var(--sp-3)' }}>{aux}</span>}
      {graph && (
        /* 文字の釦。ドックの入口と並ぶので、絵にすると 2 つの四角が
           何を開くのか見分けられない */
        <span className="k-graph-toggle">
          <Button size="chip" variant={graph.open ? 'secondary' : 'ghost'}
                  title="評価値グラフ" onClick={graph.onToggle}>グラフ</Button>
        </span>
      )}
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
      // **帯ごと桁を揃える。**思考の秒・nodes・nps は毎フレーム変わるので、
      // 桁が動くと下の帯全体が左右に揺れて読めない
      fontVariantNumeric: 'tabular-nums',
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
      {/* タブの帯は --h-field (32px)。36 は規則 5 の 5 段
          (44/32/28/24/20) に無く、絵の下部パネルも 32px。中のタブは
          --h-chip (20px) なので 32 でも上下に 6px ずつ残る */}
      <div style={{
        height: 'var(--h-field)', flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
        padding: '0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
      }}>
        {tabs.map(t => {
          const on = active === t.id;
          return (
            <button key={t.id} type="button" onClick={() => onTab?.(t.id)}
              aria-pressed={on} className={'k-press' + (on ? ' k-on' : '')}
              style={{
                height: 'var(--h-chip)', padding: '0 var(--sp-3)', border: 0, borderRadius: 'var(--r-1)', fontSize: 'var(--fs-6)',
                background: on ? 'var(--card)' : 'transparent',
                color: on ? 'var(--text)' : 'var(--sub)', fontWeight: on ? 600 : 400,
                display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
              }}>
              {t.label}
              {t.unread ? <span style={{ background: 'var(--bad)', color: 'var(--on-bad)', borderRadius: 'var(--r-pill)', padding: '0 var(--sp-1)', fontSize: 'var(--fs-7)' }}>{t.unread}</span> : null}
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
 * (Popover は使い所が無かったので削除した — 規則 70)
 *
 * **覆いは 1 つの形しか無い。** 以前は 4 枚が 4 通りの作りだった —
 * 確認は帯なしで余白 24、棋譜の読み込みと棋譜ビューアは罫つきの頭と足、
 * 設定だけ 44px の `--panel` の帯に閉じる釦。**同じ「浮いて決めさせるもの」
 * が画面ごとに違って見えていた。**
 *
 * 段は 3 つ。**地の色は 3 段で変える** (設定の窓がそうだった。実測 —
 * 題名 #1f2328 / タブ #2c3138 / 中身 #17191d):
 *   頭 … `--h-bar` (44px) の `--panel`。題名は中央、閉じるは右端 (規則 5)
 *   本体 … **`--bg` (いちばん暗い)**。余白 16/24。長ければ `scroll` で巻く
 *   足 … 頭と同じ `--panel` + 上罫。`actions` を渡したときだけ出す
 *
 * **中身を明るくしない。**帯より中身が明るいと、帯が沈んで箱の縁に見え、
 * 「浮いている 1 枚」ではなく「窓の中の板」に見える。
 *
 * 幅は `--w-modal` (340) か `--w-modal-wide` (520)。高さは中身なりで、
 * 画面が低ければ `88vh` で頭打ちにして本体だけを巻く。
 */
export function Modal({ title, sub, body, actions, width = 'var(--w-modal)', onClose, scroll, band, children }: {
  title: string;
  /** 題名の下の一行 (棋譜の読み込みの「GGF・f5d6… のいずれでも」など)。 */
  sub?: React.ReactNode;
  /** 本体。`children` と同じ扱い — 短い文だけのときはこちらが読みやすい。 */
  body?: React.ReactNode;
  actions?: React.ReactNode;
  /** 既定は `--w-modal` (340px)。中身の要る覆いは `--w-modal-wide` (規則 71) */
  width?: string;
  /** 閉じる。渡すと頭の右端に釦が出る。 */
  onClose?: () => void;
  /** 本体を巻けるようにする (設定のように中身が長いもの)。 */
  scroll?: boolean;
  /** 頭のすぐ下に**固定**で置く帯 (設定のタブ)。**本体と一緒に巻かない** —
   *  巻くとタブが上へ流れて、いまどの枚を見ているのか分からなくなる。 */
  band?: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <div role="dialog" aria-modal style={{
      width, maxHeight: '88vh',
      borderRadius: 'var(--r-4)', background: 'var(--bg)',
      border: '1px solid var(--border)', boxShadow: 'var(--sh-2)',
      display: 'flex', flexDirection: 'column', overflow: 'hidden',
    }}>
      <div style={{
        height: 'var(--h-bar)', flex: 'none', background: 'var(--panel)',
        borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center',
        padding: '0 var(--sp-2)',
      }}>
        {/* 題名を**窓の中央**に置くため、閉じると同じ幅を左にも取る */}
        <span style={{ width: 32, flex: 'none' }} />
        <div style={{ flex: 1, minWidth: 0, textAlign: 'center' }}>
          <div style={{
            fontSize: 'var(--fs-4)', fontWeight: 600, color: 'var(--text)',
            overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}>{title}</div>
          {/* 副題は**頭の中**、題名の下 (絵の §9 がこの形)。本体に置くと
              中身の 1 行目と見分けが付かない */}
          {sub && <div style={{
            fontSize: 'var(--fs-7)', color: 'var(--sub)',
            overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}>{sub}</div>}
        </div>
        <span style={{ width: 32, flex: 'none', display: 'grid', placeItems: 'center' }}>
          {onClose && <IconButton name="close" label="閉じる" onClick={onClose} />}
        </span>
      </div>

      {/* 帯の器は**こちらが持つ** — 高さも地も罫も覆いの決まりなので、
          中身の側に書かせると画面ごとにずれる */}
      {band && (
        <div style={{
          flex: 'none', height: 'var(--h-bar)', background: 'var(--card)',
          borderBottom: '1px solid var(--border)', padding: '0 var(--sp-4)',
          display: 'flex', alignItems: 'center',
        }}>{band}</div>
      )}

      <div className={scroll ? 'k-scroll' : undefined} style={{
        flex: scroll ? 1 : 'none', minHeight: 0, background: 'var(--bg)',
        padding: 'var(--sp-4) var(--sp-5)',
        display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)',
      }}>
        {body && <div style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)', lineHeight: 1.7 }}>{body}</div>}
        {children}
      </div>

      {actions && (
        <div style={{
          flex: 'none', display: 'flex', gap: 'var(--sp-2)', alignItems: 'center',
          padding: 'var(--sp-3) var(--sp-5)', borderTop: '1px solid var(--border)',
          background: 'var(--panel)',
        }}>{actions}</div>
      )}
    </div>
  );
}

/* 節の中に並べる一覧。**`Section` の直下に行を置かないこと。**
 *
 * `Section` は節の中身を `--sp-3` (12px) の溝で積む。説明文や欄が並ぶ節では
 * それでよいが、**一覧の行の間に 12px が入ると箇条書きに見える** —
 * 24px の行が 36px 間隔、32px の行が 44px 間隔で並ぶ。
 * 同じ取りこぼしを 4 回踏んだ (定石の木 / 学習ログ / GGS の 3 つ) ので、
 * 名前のある器にした。行を並べるときは必ずこれで包む。
 */
export function List({ children }: { children: React.ReactNode }) {
  return <div style={{ display: 'flex', flexDirection: 'column' }}>{children}</div>;
}

/** ツールバーの縦罫。**押す操作と前提を決めるものを切る。**
 *  切らないと「新規対局」と「黒」が同じ並びに見える。
 *  `App.tsx` に同じ 4 行が並んでいたのでまとめた。 */
export function Divider() {
  return <span style={{
    width: 1, height: 20, flex: 'none',
    margin: '0 var(--sp-1)', background: 'var(--border)',
  }} />;
}

/** 節や欄に添える説明文。**規則 73 — 読み物は `--w-text` (720px) で
 *  折り返す。** 表・一覧・盤は窓いっぱいでよいが、文章の 1 行が 1500px に
 *  なると目が行頭に戻れない。7 か所で同じ体裁を手書きしていた。 */
export function Note({ children }: { children: React.ReactNode }) {
  return <p style={{
    margin: 0, maxWidth: 'var(--w-text)',
    fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8,
  }}>{children}</p>;
}

/* 表の列見出しの行。**4 か所で同じ体裁を手書きしていた** (棋譜の表 /
 * 学習ログ / プレイヤーの一覧 / 定石の木) — 字も高さも罫も同じなのに
 * 書いた場所ごとに並び順が違い、直すときに見落とす。
 *
 * 高さは `--h-head` (20px)、字は節の見出しと同じ (`--fs-7` / 600 /
 * 字間 .08em / `--sub`)。**列そのものは呼ぶ側が `<span>` の幅で並べる** —
 * 幅はその表だけの数字なので、部品に持たせても揃わない。 */
export function TableHead({ pad = 'var(--sp-3)', children }: {
  /** 左右の余白。行の余白と揃える (既定は `--sp-3`)。 */
  pad?: string;
  children: React.ReactNode;
}) {
  return (
    <div style={{
      flex: 'none', height: 'var(--h-head)', display: 'flex', alignItems: 'center',
      gap: 'var(--sp-2)', padding: `0 ${pad}`,
      borderBottom: '1px solid var(--border)',
      fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)',
    }}>{children}</div>
  );
}

/** 選ばれている行の見せ方。**accent 14% の面 + 左に 2px。**
 *
 * 一覧は形が 3 通りある (表の行 / 会話の 2 段 / 手合いの 2 段) が、
 * **「選ばれている」の見せ方だけは 1 つ**にする。値を手書きで散らして
 * いたせいで一度ずれた (学習ログだけ `--card` の面 + 角丸だった)。 */
export const picked = (on: boolean): React.CSSProperties => ({
  background: on ? 'color-mix(in srgb, var(--accent) 14%, transparent)' : 'transparent',
  boxShadow: on ? 'inset 2px 0 0 var(--accent)' : 'none',
});

/* 表の 1 行。**同じ形を 3 か所で手書きしていた** (棋譜の表 / 学習ログ /
 * プレイヤーの一覧)。高さ `--h-row` (24px)、下に 1px の罫、選ばれている行は
 * **accent 14% の面 + 左に 2px** — この「選ばれている」の見せ方が画面ごとに
 * 違っていて、一度直したのにまた別の場所でずれた。名前のある器にする。
 *
 * **列そのものは呼ぶ側が `<span>` の幅で並べる** (`TableHead` と同じ理由)。 */
export function TableRow({ on, pad = 'var(--sp-3)', fs = 'var(--fs-5)', muted, onClick, title, innerRef, children }: {
  /** 選ばれている行。 */
  on?: boolean;
  /** 左右の余白。列見出しと揃える。 */
  pad?: string;
  /** 字の大きさ。狭い列では `--fs-6` にする。 */
  fs?: string;
  /** まだ値の無い行 (棋譜の未着手) は弱く出す。 */
  muted?: boolean;
  onClick?: () => void;
  title?: string;
  /** 現在行を追いかけるための ref (棋譜の表が使う)。 */
  innerRef?: React.Ref<HTMLButtonElement>;
  children: React.ReactNode;
}) {
  return (
    <button type="button" onClick={onClick} title={title} ref={innerRef}
      aria-current={on || undefined}
      className={'k-row' + (on ? ' k-on' : '')}
      style={{
        width: '100%', border: 0, textAlign: 'left',
        display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
        height: 'var(--h-row)', padding: `0 ${pad}`, fontSize: fs,
        // 表の行は必ず数字の列を持つ。桁が揃わないと縦に読み比べられない
        fontVariantNumeric: 'tabular-nums',
        borderBottom: '1px solid var(--border-weak)',
        color: muted ? 'var(--sub)' : 'var(--text)',
        ...picked(!!on),
      }}>{children}</button>
  );
}

/* **一覧が空のときの一言。**節や表の中に置く小さいほう。
 *
 * 画面まるごとが空なら `EmptyState` (絵と題名と行き先を持つ大きいほう)。
 * **この 2 つ以外を書かない** — 生の `<span>` で書くと、色も余白も
 * 書いた場所ごとに変わる。実際に 4 か所が別々の見た目になっていた
 * (棋譜の表 / 学習ログ / 定石の木 / チャット)。 */
export function Empty({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ padding: 'var(--sp-3) 0', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
      {children}
    </div>
  );
}

/* 押せない盤を出さないための空状態。GGS 未対局時など */
export function EmptyState({ title, body, actions }: { title: string; body?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <div style={{ flex: 1, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)' }}>
      <div style={{ maxWidth: 420, textAlign: 'center', display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 'var(--sp-5)' }}>
        <span aria-hidden style={{ width: 64, height: 64, borderRadius: 'var(--r-4)', overflow: 'hidden', opacity: .5, display: 'block' }}
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
/** 焦点を置ける相手 (Tab の輪に入るもの)。 */
const FOCUSABLE =
  'textarea:not(:disabled), input:not(:disabled), button:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])';
/** 開いたときに焦点を置きたい相手。**打つ場所を釦より先に**する —
 *  `querySelectorAll` は書いた順ではなく**文書の順**で返すので、
 *  頭の閉じる釦が先に来てしまう (棋譜の読み込みで貼り付け先ではなく
 *  「閉じる」に入っていた)。 */
const FIRST_STOP = 'textarea:not(:disabled), input:not(:disabled), select:not(:disabled)';

export function Overlay({ onClose, children }: { onClose?: () => void; children: React.ReactNode }) {
  React.useEffect(() => {
    if (!onClose) return;
    const on = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', on);
    return () => window.removeEventListener('keydown', on);
  }, [onClose]);

  /* **焦点を中へ入れ、輪にして、閉じたら元へ返す。**`aria-modal` を
     名乗っているのに焦点は後ろに残ったままで、Tab を押すと暗幕の下の
     押せないものを順に辿っていた。入れる先は最初の押せるもの
     (棋譜の読み込みなら貼り付け先の欄) — 無ければ器そのもの。 */
  const box = React.useRef<HTMLDivElement>(null);
  React.useEffect(() => {
    const el = box.current;
    const back = document.activeElement as HTMLElement | null;
    /** いま押せるものだけ。`offsetParent` が無いもの (畳まれた段) は数えない */
    const items = () => [...(el?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])]
      .filter((x) => x.offsetParent !== null);
    (el?.querySelector<HTMLElement>(FIRST_STOP) ?? items()[0] ?? el)?.focus();
    // 端で折り返す。輪にしないと、最後の釦から暗幕の下へ抜けてしまう
    const on = (e: KeyboardEvent) => {
      if (e.key !== 'Tab' || !el) return;
      const list = items();
      if (!list.length) return;
      const a = document.activeElement;
      const out = !el.contains(a);
      if (e.shiftKey ? (out || a === list[0]) : (out || a === list[list.length - 1])) {
        e.preventDefault();
        (e.shiftKey ? list[list.length - 1] : list[0]).focus();
      }
    };
    window.addEventListener('keydown', on);
    return () => { window.removeEventListener('keydown', on); back?.focus?.(); };
  }, []);

  return (
    <div onClick={onClose} style={{
      position: 'absolute', inset: 0, zIndex: 30,
      background: 'var(--scrim)', display: 'grid', placeItems: 'center',
      padding: 'var(--sp-5)',
    }}>
      {/* 中身を押したときに閉じない */}
      <div ref={box} tabIndex={-1} onClick={(e) => e.stopPropagation()}>{children}</div>
    </div>
  );
}
