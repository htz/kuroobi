import React from 'react';
/* IconButton は Icons.tsx のものを使う（primitives 側は削除済み）。
 * props は {name, label, onClick, size} — label が title と aria-label の両方になり、
 * onClick は引数を取らない。 */
import { Icon, IconButton, type IconName } from './Icons';
import type { GgsSnapshot } from '../types';
import { Badge, Dot, Button, Select, TextField } from './primitives';
import { Note, picked } from './layout';
import logo from '../../assets/kuroobi.svg?raw';
// 値の実体は 1 つ。設計側の state.ts に写しがあったが、定数が 2 か所にあると
// 必ず割れる（レベル表・条件式の変数・色の値で実際に割れていた）
import {
  COLOR_CHOICES, BOOL_OPS, FORMULA_OPS, FORMULA_VARS, varOf, condToSrc, condLabel,
  isGroup, type Cond as SharedCond, type FormulaOp,
} from '../ggs';

/* KUROOBI GGS 専用の部品
 * 左メニュー / 資源メーター / 手合い一覧 / レート / 条件式の木 /
 * 翻訳つき吹き出し / 通信ログ / トースト。
 */

/* ============ 左メニュー ============ */

export type NavId =
  | 'play' | 'study' | 'book'
  | 'ggs-play' | 'ggs-lobby' | 'ggs-players' | 'ggs-results' | 'ggs-chat' | 'ggs-standby' | 'ggs-console' | 'ggs-settings'
  | 'ggs-login';

export type NavItem = {
  id: NavId;
  label: string;
  icon: IconName;     // Icons.tsx の IconName
  count?: number;    // 件数バッジ
  alert?: boolean;   // 自分宛・未読は --bad で塗る
  dot?: 'ok' | 'gold' | 'bad';  // 状態バッジ（待機モードが有効・自分の手番 等）
  /** 鍵。**押せることは見れば分かるが、鍵があることは押しても分からない**
   *  ので title に添える (対局の釦が `title="⌘N"` としているのと同じ)。
   *  畳んだ 48px の列では文字が消えるぶん、title の値打ちが上がる */
  hint?: string;
};

export type Conn = 'offline' | 'connecting' | 'logging-in' | 'online';

const CONN_TONE: Record<Conn, 'sub' | 'gold' | 'ok'> = {
  offline: 'sub', connecting: 'gold', 'logging-in': 'gold', online: 'ok',
};

export const NAV_LOCAL: NavItem[] = [
  { id: 'play', label: '対局', icon: 'play' },
  { id: 'study', label: '検討', icon: 'study' },
  { id: 'book', label: '定石', icon: 'book', hint: '⌘B' },
];

/* 未接続のときは 7 行を出さず「ログイン」1 行だけにする */
export function ggsNav(conn: Conn, badges?: Partial<Record<NavId, Pick<NavItem, 'count' | 'alert' | 'dot'>>>): NavItem[] {
  if (conn !== 'online') return [{ id: 'ggs-login', label: 'ログイン', icon: 'login' }];
  const base: NavItem[] = [
    { id: 'ggs-play', label: '対局・観戦', icon: 'ggs-play' },
    { id: 'ggs-lobby', label: 'ロビー', icon: 'ggs-lobby' },
    { id: 'ggs-players', label: 'プレイヤー', icon: 'ggs-users' },
    { id: 'ggs-results', label: '結果', icon: 'results' },
    { id: 'ggs-chat', label: 'チャット', icon: 'ggs-chat' },
    { id: 'ggs-standby', label: '待機モード', icon: 'ggs-standby' },
    { id: 'ggs-console', label: 'コンソール', icon: 'ggs-console' },
    // GGS の設定は設定の窓の「GGS」タブへ移した。行き先を 2 つ持つと、
    // どちらが本物か分からなくなる (規則 58)
  ];
  return base.map(i => ({ ...i, ...(badges?.[i.id] ?? {}) }));
}

export function Nav({ items, ggsItems, conn, active, onSelect, footer }: {
  items: NavItem[]; ggsItems: NavItem[]; conn: Conn;
  active: NavId; onSelect?: (id: NavId) => void; footer?: React.ReactNode;
}) {
  return (
    // width は base.css の .k-nav が持つ（1040px で畳むので、インラインだと届かない）
    <nav className="k-nav" style={{
      flex: 'none', background: 'var(--panel)',
      borderRight: '1px solid var(--border)', display: 'flex', flexDirection: 'column', minHeight: 0,
    }}>
      {/* ロゴの帯。信号機ぶんの 78px は窓の帯が持つので、ここには要らない。
          畳む段では丸ごと落とす (48px の列に文字のロゴは入らない) */}
      <div className="k-nav-logo" style={{ height: 'var(--h-bar)', flex: 'none' }}>
        <span aria-label="KUROOBI" dangerouslySetInnerHTML={{ __html: logo }} />
      </div>
      <div style={{ padding: 'var(--sp-1) var(--sp-2) 0', display: 'flex', flexDirection: 'column', gap: 1 }}>
        {items.map(i => <NavRow key={i.id} item={i} active={active === i.id} onSelect={onSelect} />)}
      </div>
      {/* display は .k-nav-section が持つ（1040px で消す） */}
      {/* 畳んでも区切りは残す — 文字は落とすが 1px の罫は残して、
          行き先の 2 つの束が地続きに見えないようにする (.k-nav-rule) */}
      <div className="k-nav-rule" />
      <div className="k-nav-section" style={{
        margin: 'var(--sp-4) 0 6px', padding: '0 18px', alignItems: 'center', gap: 'var(--sp-2)',
        fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.1em', color: 'var(--sub)',
      }}>
        <Dot tone={CONN_TONE[conn]} />GGS
      </div>
      <div style={{ padding: '0 var(--sp-2)', display: 'flex', flexDirection: 'column', gap: 1 }}>
        {ggsItems.map(i => <NavRow key={i.id} item={i} active={active === i.id} onSelect={onSelect} />)}
      </div>
      {/* padding は畳む段が動かすので .k-nav-foot が持つ。48px の列で
          sp-3 (12px) を左右に取ると中身が 24px しか残らず、32px の当たりが
          はみ出す */}
      {footer && <div className="k-nav-foot" style={{
        marginTop: 'auto', display: 'flex', flexDirection: 'column',
        gap: 'var(--sp-3)', borderTop: '1px solid var(--border)',
      }}>{footer}</div>}
    </nav>
  );
}

function NavRow({ item, active, onSelect }: { item: NavItem; active: boolean; onSelect?: (id: NavId) => void }) {
  return (
    <button type="button" onClick={() => onSelect?.(item.id)} aria-current={active || undefined}
      title={item.hint ? `${item.label} (${item.hint})` : item.label}
      className={'k-nav-row k-row' + (active ? ' k-on' : '')}
      // padding / justify-content は .k-nav-row が持つ（1040px で中央寄せにする）
      style={{
        height: 'var(--h-field)', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        border: 0, borderRadius: 'var(--r-2)', fontSize: 'var(--fs-4)',
        background: active ? 'var(--accent-dim)' : 'transparent',
        color: active ? 'var(--on-accent)' : 'var(--text)', fontWeight: active ? 600 : 400,
      }}>
      {/* **絵は縮ませない。** 48px の列では 絵 16 + 溝 12 + バッジ 18 = 46 で
          32px の当たりに収まらず、`flex` が絵のほうを 0 まで潰していた
          (数字だけが残ってどの行か分からなくなる) */}
      <span style={{ flex: 'none', display: 'grid', placeItems: 'center' }}>
        <Icon name={item.icon} size={16} />
      </span>
      <span className="k-nav-label">{item.label}</span>
      {/* **置き場所も大きさも base.css が持つ。** 畳む段で絵の角へ移して
          小さくするので、ここに書くと media query が届かない。
          色と太さだけがここの持ち物 */}
      {item.count != null && (
        <span className="k-nav-n" style={{
          fontWeight: item.alert ? 700 : 600, display: 'grid', placeItems: 'center',
          background: item.alert ? 'var(--bad)'
            : active ? 'color-mix(in srgb, var(--on-accent) 22%, transparent)' : 'var(--border)',
          color: item.alert ? 'var(--on-bad)' : active ? 'var(--on-accent)' : 'var(--text)',
        }}>{item.count}</span>
      )}
      {item.dot && <span className="k-nav-dot" style={{
        width: 7, height: 7, borderRadius: '50%', background: 'var(--' + item.dot + ')',
      }} />}
    </button>
  );
}

/* ============ 資源メーター ============
 * ローカル探索と GGS 対局は別スレッドプールで動くので、
 * 何が走っているかが常に見えないと切り分けられない。
 */

export function Meter({ icon, label, value, unit, ratio, note }: {
  icon: IconName; label: string; value: React.ReactNode; unit?: string; ratio: number; note?: string;
}) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
      {/* 48px の列に残すのは絵と 4px の溝だけ。文字は .k-meter-text が
          1040px で落とす。**絵はその外に出す** — 中に入れると一緒に消えて、
          何の溝なのか分からない棒が 2 本並ぶ */}
      <div className="k-meter-head" style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        <Icon name={icon} size={13} />
        <span className="k-meter-text">{label}</span>
        <span className="k-meter-text" style={{ marginLeft: 'auto', color: 'var(--text)' }}>
          <b style={{ fontWeight: 600 }}>{value}</b>{unit && <span style={{ color: 'var(--sub)' }}>{unit}</span>}
        </span>
      </div>
      {/* 溝は 48px の列では落とす (.k-meter-bar)。4px の棒が 2 本並んでも
          読めるのは「多いか少ないか」だけで、数字のほうが要る */}
      <div className="k-meter-bar" style={{ height: 4, borderRadius: 'var(--r-0)', background: 'var(--track)', overflow: 'hidden' }}>
        <span style={{
          display: 'block', width: Math.min(100, Math.round(ratio * 100)) + '%', height: '100%',
          background: ratio > 0.75 ? 'var(--gold)' : 'var(--accent)',
        }} />
      </div>
      {/* 畳んだ列ではこちらだけが出る。絵の下に数字 1 つ */}
      <div className="k-meter-mini" style={{
        fontSize: 'var(--fs-7)', color: 'var(--text)', textAlign: 'center',
        fontVariantNumeric: 'tabular-nums',
      }}>{value}{typeof unit === 'string' ? unit.trim() : unit}</div>
      {note && <div style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{note}</div>}
    </div>
  );
}

/* 走っている仕事。譲り中（GGS 対局を優先して学習を止めている）も出す */
export function JobList({ jobs }: { jobs: { label: string; threads?: number; yielded?: boolean }[] }) {
  // 中身は文字だけなので、48px の列では丸ごと落とす (.k-nav-jobs)
  if (!jobs.length) return <div className="k-nav-jobs" style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>待機中</div>;
  // display は .k-nav-jobs が持つ（1040px で消す）。インラインに書くと届かない
  return (
    <div className="k-nav-jobs" style={{ flexDirection: 'column', gap: 3, fontSize: 'var(--fs-7)', color: 'var(--text)' }}>
      {jobs.map((j, i) => (
        <div key={i}>{j.label}{j.threads ? ` (${j.threads}スレ)` : ''}
          {j.yielded && <span style={{ color: 'var(--sub)' }}> 譲り中</span>}
        </div>
      ))}
    </div>
  );
}

/* ============ レート ============ */

export type Rate = { value: number; dev: number; rank?: number; w: number; l: number; d: number; provisional?: boolean };

/* 偏差 ±n は必ず添える。GGS のレートは偏差が大きいと数字が意味を持たない */
export function RateRow({ label, rate }: { label: string; rate: Rate }) {
  return (
    <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)', fontVariantNumeric: 'tabular-nums' }}>
      {/* 対局形式の名前は設定の見出しより長い (「同期・ランダム20手」)。
          --w-label のままだと 2 行に折り返して行の高さが揃わなくなる */}
      <span style={{
        width: 'var(--w-gtype)', flex: 'none', whiteSpace: 'nowrap',
        fontSize: 'var(--fs-6)', color: 'var(--sub)',
      }}>{label}</span>
      <span style={{ fontSize: 'var(--fs-1)', fontWeight: 700 }}>{rate.value.toFixed(1)}</span>
      <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        ±{rate.dev}{rate.provisional && <span style={{ color: 'var(--gold)', marginLeft: 5 }}>暫定</span>}
      </span>
      {rate.rank != null && <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{rate.rank} 位</span>}
      <span style={{ fontSize: 'var(--fs-6)' }}>
        <span style={{ color: 'var(--ok)' }}>{rate.w}勝</span>{' '}
        <span style={{ color: 'var(--bad)' }}>{rate.l}敗</span> {rate.d}分
      </span>
    </div>
  );
}

/* 対局者行と石の点は data.tsx が持つ（PlayerRow / StoneDot）。
 * 同名の部品を 2 ファイルに置くと import 元を間違えるし、
 * 色の型が 1|2 と 'b'|'w' で割れていた。画面の色は 'b' | 'w' に統一。 */
export { PlayerRow, StoneDot, toStoneColor, type StoneColor } from './data';

/* ============ 手合い一覧 ============
 * 同期対局は 2 局で 1 組。組を 1 行として扱う。
 */

export type Match = {
  id: string; mine: boolean; live: boolean;
  /** 自分の対局なら相手名。観戦なら black / white を使う */
  opponent?: string;
  black: string; white: string;
  kind: string;      // 同期・ランダム16手 / 通常
  boards: number;    // 同期なら 2
  ply: number;
  myTurn?: boolean;
  result?: string;   // 終局時 +8 等
};

const matchTitle = (m: Match) =>
  m.mine ? `自分 対 ${m.opponent ?? '?'}` : `${m.black} 対 ${m.white}`;

export function MatchRow({ m, active, onSelect, onClose }: {
  m: Match; active?: boolean; onSelect?: () => void; onClose?: () => void;
}) {
  const closable = !m.live && !!onClose;
  // 行の本体と閉じるボタンは兄弟にする。button の入れ子は HTML として無効で、
  // IconButton も使えなくなる（role / tabIndex / onKeyDown を手で書き直す羽目になる）。
  // hover は外側の .k-row が持つ。重なっていないので stopPropagation も要らない。
  return (
    <div className={'k-row' + (active ? ' k-on' : '')} style={{
      position: 'relative', borderBottom: '1px solid var(--border-weak)',
      ...picked(!!active),
    }}>
      {/* 閉じるボタンは右端から 36px を占めるので、行の本体に余白を持たせる。
          持たせないと長い名前がボタンの下に潜る */}
      {/* 一覧の行なので `k-row`。付け忘れると、選ばれている行の色は出るのに
          触れても何も変わらない (押せる場所だと分からない) */}
      <button type="button" onClick={onSelect} className="k-row"
              aria-current={active || undefined} style={{
        width: '100%', border: 0, background: 'transparent', textAlign: 'left',
        padding: '9px var(--sp-3)', paddingRight: closable ? 40 : undefined,
        display: 'flex', flexDirection: 'column', gap: 5,
      }}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
          <Tag tone={m.mine ? 'accent' : 'sub'}>{m.mine ? '自分' : '観戦'}</Tag>
          <Tag tone={m.live ? 'ok' : 'sub'}>{m.live ? '対局中' : '終局'}</Tag>
          <span style={{ fontSize: 'var(--fs-5)', color: m.live ? 'var(--text)' : 'var(--sub)' }}>
            {matchTitle(m)}
          </span>
          {m.myTurn && <Dot />}
        </span>
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
          {m.kind}{m.boards > 1 && ` · ${m.boards} 局`} · {m.ply} 手目{m.result && ` · ${m.result}`}
        </span>
      </button>
      {closable && (
        <span style={{ position: 'absolute', right: 4, top: 4 }}>
          <IconButton name="close" label="一覧から閉じる" onClick={onClose!} size={14} />
        </span>
      )}
    </div>
  );
}

export function Tag({ tone = 'sub', children }: { tone?: 'sub' | 'accent' | 'ok' | 'bad'; children: React.ReactNode }) {
  const solid = tone === 'accent';
  return <span style={{
    padding: '1px 6px', borderRadius: 'var(--r-1)', fontSize: 'var(--fs-tag)', flex: 'none',
    fontWeight: solid ? 600 : 400,
    background: solid ? 'var(--accent-dim)' : tone === 'sub' ? 'var(--card)' : `color-mix(in srgb, var(--${tone}) 18%, transparent)`,
    color: solid ? 'var(--on-accent)' : tone === 'sub' ? 'var(--sub)' : `var(--${tone})`,
  }}>{children}</span>;
}

/* ============ 条件式の木 ============
 * GGS の /os 条件式は入れ子の論理式で、構造がそのまま意味になっている。
 * 文字で打たせず木のまま組ませる。読むだけの場所では FormulaView。
 */

export type Cond = SharedCond;
export { isGroup };

/* 読むだけの木。束（すべて満たす / 次のどれか）は 2px の縦罫と字下げで示す。
 * 一番外側だけ --accent、内側は --border。 */
export function FormulaView({ node, top }: { node: Cond; top?: boolean }) {
  if (node.kind === 'atom') return <CondChip c={node} />;
  return (
    <div style={{
      borderLeft: '2px solid var(--' + (top ? 'accent' : 'border') + ')',
      paddingLeft: 'var(--sp-2h)', display: 'flex', flexDirection: 'column', gap: 5,
    }}>
      <div style={{
        fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.06em',
        color: node.kind === 'any' ? 'var(--gold)' : 'var(--sub)',
      }}>{node.kind === 'all' ? 'すべて満たす' : '次のどれか'}</div>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 5, alignItems: 'flex-start' }}>
        {node.kids.map((n, i) => isGroup(n)
          ? <FormulaView key={i} node={n} />
          : <CondChip key={i} c={n} />)}
      </div>
    </div>
  );
}

function CondChip({ c }: { c: Cond }) {
  return (
    <span style={{
      padding: '2px var(--sp-2)', borderRadius: 'var(--r-1)', background: 'var(--panel)',
      border: '1px solid var(--border)', fontSize: 'var(--fs-6)', color: 'var(--text)', fontFamily: 'var(--ff-mono)',
    }}>{condLabel(c)}</span>
  );
}

/* 何がサーバーへ行くのかは隠さない。逃げ道として生の式も書けるようにする */
function FormulaWire({ text }: { text: string }) {
  return (
    <div style={{
      display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', padding: '9px var(--sp-3)',
      borderRadius: 'var(--r-3)', background: 'var(--panel)', border: '1px solid var(--border)',
    }}>
      <span style={{ fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)', flex: 'none' }}>送る式</span>
      <code style={{ fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', color: 'var(--text)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{text}</code>
    </div>
  );
}

/* 組む側。変数の型で 3 枠目の中身が変わる（真偽なら「である／ではない」だけ、
 * 色なら 黒 / 白 / おまかせ、数値なら 比較 6 つ ＋ 値 ＋ 単位）。
 * 木を持ったまま編集し、保存するときだけ condToSrc で文字列にする。
 *
 * value は null を取る — 条件を付けていない状態がある。そのときは「指定なし」と
 * 「条件を付ける」をこの部品が出す（親で分岐させない）。 */
export function FormulaEditor({ value, onChange, onSave, onClear, onRaw }: {
  value: Cond | null;
  onChange: (c: Cond) => void;
  onSave?: (src: string) => void;
  onClear?: () => void;
  /** 逃げ道。GGS の式を直接書く（木では表せない式が書ける） */
  onRaw?: (src: string) => void;
}) {
  if (!value) {
    return (
      <div style={{
        borderRadius: 'var(--r-3)', border: '1px solid var(--border)', background: 'var(--bg)',
        padding: 'var(--sp-4)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)', alignItems: 'flex-start',
      }}>
        <div style={{ fontSize: 'var(--fs-5)', color: 'var(--text)' }}>指定なし</div>
        <Note>申し込みは自動で処理せず、届いたときに本人が判断します。</Note>
        <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
          <Button onClick={() => onChange(newGroup())}>条件を付ける</Button>
          {onRaw && <Button variant="ghost" onClick={() => onRaw('')}>式を直接書く</Button>}
        </div>
      </div>
    );
  }
  const src = condToSrc(value);
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
      <CondNode node={value} top onChange={onChange} />
      <FormulaWire text={src} />
      <div style={{ display: 'flex', gap: 'var(--sp-2)', alignItems: 'center' }}>
        <Button variant="secondary" onClick={onClear}>条件を外す</Button>
        {onRaw && <Button variant="ghost" onClick={() => onRaw(src)}>式を直接書く</Button>}
        <span style={{ marginLeft: 'auto' }}>
          <Button variant="primary" onClick={() => onSave?.(src)}>反映する</Button>
        </span>
      </div>
    </div>
  );
}

const newAtom = (): Cond => ({ kind: 'atom', name: 'rated', op: '=', val: '', neg: false });
/* 束は必ず条件 1 つを入れた状態で作る — 中身ゼロの空の枠を
 * 置いても、押した直後に次の操作が見えない。 */
const newGroup = (op: 'all' | 'any' = 'all'): Cond => ({ kind: op, kids: [newAtom()] });

function CondNode({ node, top, onChange, onRemove }: {
  node: Cond; top?: boolean; onChange: (c: Cond) => void; onRemove?: () => void;
}) {
  if (node.kind === 'atom') return <AtomRow c={node} onChange={onChange} onRemove={onRemove} />;

  const setKid = (i: number, c: Cond | null) => {
    const kids = c ? node.kids.map((k, j) => (j === i ? c : k)) : node.kids.filter((_, j) => j !== i);
    onChange({ ...node, kids });
  };
  const addAtom = () => onChange({ ...node, kids: [...node.kids, newAtom()] });
  const addGroup = () => onChange({ ...node, kids: [...node.kids, newGroup('any')] });

  return (
    <div style={{
      borderRadius: 'var(--r-3)', border: '1px solid var(--border)', background: 'var(--bg)',
      padding: 'var(--sp-3)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)',
    }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
        <Select value={node.kind} width={132} size="ctrl"
                options={[['all', 'すべて満たす'], ['any', '次のどれか']]}
                onChange={v => onChange({ ...node, kind: v as 'all' | 'any' })} />
        {onRemove && (
          <span style={{ marginLeft: 'auto' }}>
            <IconButton name="close" label="この束を取り除く" onClick={onRemove} size={13} />
          </span>
        )}
      </div>
      <div style={{
        paddingLeft: 12, borderLeft: '2px solid var(--' + (top ? 'accent' : 'border') + ')',
        display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
      }}>
        {node.kids.map((k, i) => (
          <CondNode key={i} node={k} onChange={c => setKid(i, c)} onRemove={() => setKid(i, null)} />
        ))}
        <div style={{ display: 'flex', gap: 'var(--sp-2)', paddingTop: 2 }}>
          <Button onClick={addAtom}>+ 条件</Button>
          <Button onClick={addGroup}>+ 束</Button>
        </div>
      </div>
    </div>
  );
}

function AtomRow({ c, onChange, onRemove }: {
  c: Extract<Cond, { kind: 'atom' }>; onChange: (c: Cond) => void; onRemove?: () => void;
}) {
  const v = varOf(c.name);
  const type = v?.type ?? 'bool';
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
      <Select value={c.name} width={140} size="ctrl"
              options={FORMULA_VARS.map(x => [x.name, x.label] as [string, string])}
              onChange={name => {
                const nv = varOf(name);
                onChange({
                  ...c, name,
                  // 型が変われば比較と値も作り直す（数値の既定値は変数が持つ）
                  op: nv?.type === 'num' ? '≥' : '=',
                  val: nv?.type === 'num' ? String(nv.def ?? 0) : nv?.type === 'color' ? '?' : '',
                });
              }} />
      {type === 'bool' && (
        <Select value={c.neg ? '1' : '0'} width={88} size="ctrl"
                options={BOOL_OPS.map(([neg, label]) => [neg ? '1' : '0', label] as [string, string])}
                onChange={s => onChange({ ...c, neg: s === '1' })} />
      )}
      {type === 'color' && <>
        {/* 真偽と同じ語に揃える — 「が」「でない」だけでは単体で読めない */}
        <Select value={c.op} width={88} size="ctrl"
                options={[['=', 'である'], ['≠', 'ではない']]}
                onChange={op => onChange({ ...c, op: op as FormulaOp })} />
        <Select value={c.val} width={84} size="ctrl" options={COLOR_CHOICES}
                onChange={val => onChange({ ...c, val })} />
      </>}
      {type === 'num' && <>
        <Select value={c.op} width={74} size="ctrl"
                options={FORMULA_OPS.map(o => [o, o] as [string, string])}
                onChange={op => onChange({ ...c, op: op as FormulaOp })} />
        <TextField value={c.val} numeric align="right" width={58}
                   onChange={val => onChange({ ...c, val })} />
        {v?.unit && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{v.unit}</span>}
      </>}
      {onRemove && (
        <span style={{ marginLeft: 'auto' }}>
          <IconButton name="close" label="この条件を取り除く" onClick={onRemove} size={13} />
        </span>
      )}
    </div>
  );
}

/* ============ チャット ============ */

export type Msg = { from: string; mine?: boolean; at: string; body: string; ja?: string };

/* 英語の発言には和訳を添える（原文の下に 1px 罫で区切って小さく） */
export function Bubble({ m, showName }: { m: Msg; showName?: boolean }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 3, alignItems: m.mine ? 'flex-end' : 'flex-start' }}>
      {showName && (
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 7 }}>
          <span className="k-sel" style={{ fontSize: 'var(--fs-6)', fontWeight: 600, color: m.mine ? 'var(--accent)' : 'var(--text)' }}>
            {m.mine ? '自分' : m.from}
          </span>
          <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{m.at}</span>
        </div>
      )}
      <div style={{
        maxWidth: 320, padding: '6px 10px', borderRadius: 'var(--r-bubble)', fontSize: 'var(--fs-5)', lineHeight: 1.5,
        background: m.mine ? 'var(--accent-dim)' : 'var(--panel)', color: m.mine ? 'var(--on-accent)' : 'var(--text)',
      }}>
        {/* 引用するので選べる (k-sel)。和訳も同じ */}
        <span className="k-sel">{m.body}</span>
        {m.ja && <div className="k-sel" style={{
          marginTop: 4, paddingTop: 4, borderTop: '1px solid var(--border)',
          fontSize: 'var(--fs-6)', color: 'var(--sub)',
        }}>{m.ja}</div>}
      </div>
    </div>
  );
}

export function DayMark({ children }: { children: React.ReactNode }) {
  return <div style={{
    alignSelf: 'center', fontSize: 'var(--fs-7)', color: 'var(--sub)',
    padding: '2px 10px', borderRadius: 'var(--r-pill)', background: 'var(--panel)',
  }}>{children}</div>;
}

/* ============ 通信ログ ============
 * 送信行だけ › と --accent。受信は字下げを揃えて等幅の桁を崩さない。
 */

export type LogLine = { dir: 'out' | 'in' | 'app'; text: string };

export function ConsoleLog({ lines }: { lines: LogLine[] }) {
  // 末尾付近を見ているときだけ追従する (遡って読んでいる最中は動かさない)
  const box = React.useRef<HTMLDivElement>(null);
  const stick = React.useRef(true);
  React.useEffect(() => {
    const b = box.current;
    if (b && stick.current) b.scrollTop = b.scrollHeight;
  }, [lines.length]);
  return (
    <div className="k-scroll" ref={box}
      onScroll={() => {
        const b = box.current;
        if (b) stick.current = b.scrollTop + b.clientHeight >= b.scrollHeight - 30;
      }}
      style={{
      flex: 1, minHeight: 0, padding: '10px var(--sp-3)', display: 'flex', flexDirection: 'column', gap: 3,
      fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', lineHeight: 1.7,
    }}>
      {lines.map((l, i) => (
        <div key={i} style={{
          display: 'flex', gap: 'var(--sp-2)',
          color: l.dir === 'out' ? 'var(--accent)' : l.dir === 'app' ? 'var(--sub)' : 'var(--text)',
        }}>
          <span style={{ flex: 'none', width: 8 }}>{l.dir === 'out' ? '›' : ''}</span>
          {/* 不具合を報告するときに貼るので選べる (k-sel) */}
          <span className="k-sel">{l.text}</span>
        </div>
      ))}
    </div>
  );
}

/* ============ トースト ============
 * 出るのは「失敗」と「押したのに進まない理由」だけ。
 * 作業が進んでいることの報せ・押した本人が見れば分かること・
 * エンジンの内部符丁（stopped 等）は入れない。
 */

export type Toast = { id: string; tone: 'bad' | 'gold'; text: string };

export function Toasts({ items, onDismiss }: { items: Toast[]; onDismiss?: (id: string) => void }) {
  return (
    <div style={{
      position: 'absolute', right: 'var(--sp-4)', bottom: 'calc(var(--h-status) + var(--sp-3))',
      display: 'flex', flexDirection: 'column', gap: 'var(--sp-2h)', alignItems: 'flex-end', zIndex: 20,
    }}>
      {items.map(t => (
        <button key={t.id} type="button" onClick={() => onDismiss?.(t.id)} className="k-press" style={{
          // 浮くものなので角丸は --r-4 と影 (規則 13)。裸の px を書かない (規則 1)
          maxWidth: 340, padding: 'var(--sp-3)', borderRadius: 'var(--r-4)', textAlign: 'left',
          background: 'var(--card)', border: '1px solid var(--border)', boxShadow: 'var(--sh-2)',
          fontSize: 'var(--fs-5)', lineHeight: 1.6, color: 'var(--text)',
          display: 'flex', gap: 'var(--sp-2)', alignItems: 'flex-start',
        }}>
            <span style={{ color: 'var(--' + t.tone + ')', flex: 'none', marginTop: 1 }}><Icon name="alert" size={15} /></span>
          {t.text}
        </button>
      ))}
    </div>
  );
}

/* ============ ステータスバー右端 ============
 * GGS の対局中だけ チャット / コンソール を出す。
 */

export function StatusChip({ label, unread, active, onClick }: {
  label: string; unread?: number; active?: boolean; onClick?: () => void;
}) {
  return (
    <button type="button" onClick={onClick} className={'k-press' + (active ? ' k-on' : '')} style={{
      height: 'var(--h-chip)', padding: '0 10px', borderRadius: 'var(--r-1)', background: 'transparent',
      border: '1px solid var(--' + (active ? 'accent' : 'border') + ')',
      color: 'var(--' + (active ? 'accent' : 'sub') + ')',
      fontSize: 'var(--fs-7)', display: 'flex', alignItems: 'center', gap: 'var(--sp-0)',
    }}>
      {label}
      {unread ? <Badge tone="bad">{unread}</Badge> : null}
    </button>
  );
}

/* ============ GGS の状態の帯 ============
 *
 * 設計の GGS 画面は上端に「自分の名前・両プールのレート・いまの強さ・
 * 待機モード」を並べている。**GGS で打つときに知りたいのはこの 4 つ**で、
 * どれも別の画面へ行かないと分からなかった (レートはプレイヤー、強さと
 * 待機モードは GGS の設定)。
 *
 * ツールバーの `aux` に置くので 940px 以下では落ちる — 落ちて困る操作は
 * 無く、全部ただの表示 (規則 8)。
 */
export function GgsStatus({ snap, showStrength = true }: {
  snap: GgsSnapshot;
  /** 強さを出すか。**GGS の設定の画面では出さない** — すぐ下に同じものを
   *  変える操作があり、どちらが本物か分からなくなる (規則 58)。 */
  showStrength?: boolean;
}) {
  const e = snap.engine;
  return (
    <>
      {snap.my_ranks.map((r) => (
        <span key={r.gtype} style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
          {POOL_LABEL[r.gtype] ?? r.gtype}{' '}
          <b style={{ color: 'var(--text)', fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>
            {r.rating.toFixed(0)}
          </b>
          {/* レートには必ず偏差を添える (規則 29) */}
          <span style={{ opacity: .7, marginLeft: 3 }}>±{Math.round(r.dev)}</span>
        </span>
      ))}
      {showStrength && (
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
          強さ <b style={{ color: 'var(--text)', fontWeight: 600 }}>
            深さ{e.depth} / 読切{e.solve}{e.band > 0 ? ` / 選択読み+${e.band}` : ''}
          </b>
        </span>
      )}
      {snap.standby.enabled && <Tag tone="ok">待機モード</Tag>}
    </>
  );
}

/** レートプールの短い名前。GGS の 8 / 8r の 2 つだけ。 */
const POOL_LABEL: Record<string, string> = { '8': '通常', '8r': 'ランダム' };
