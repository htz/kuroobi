// 右の設定パネル。石数・思考時間・エンジンの設定・棋譜。
import { LEVELS } from '../state';
import type { Game } from '../state';
import { Icon } from './Icons';
import { Kifu } from './Kifu';
import { Strength } from './Strength';
import type { GraphPoint } from './Graph';
import type { SearchStat } from '../types';

const fmtSecs = (v: number): string =>
  v >= 60 ? `${Math.floor(v / 60)} 分 ${(v % 60).toFixed(0)} 秒` : `${v.toFixed(1)} 秒`;

/// 桁が 4 つも 5 つも動くので、単位を繰り上げて 3 桁前後に収める。
/// 生の桁数を出すと、伸びているのか止まっているのかがかえって読めない。
const fmtNodes = (v: number): string =>
  v >= 1e9 ? `${(v / 1e9).toFixed(1)}G`
    : v >= 1e6 ? `${(v / 1e6).toFixed(1)}M`
    : v >= 1e3 ? `${(v / 1e3).toFixed(0)}k`
    : `${Math.round(v)}`;

export interface PanelProps {
  g: Game;
  /** 評価値グラフの計算結果 (棋譜の表にも出す)。 */
  gvals?: (GraphPoint | undefined)[] | null;
  onSave: () => void;
  onLoad: () => void;
}

/** 2〜4 択の切り替え。画面の設定はすべてこの形に揃えてある。 */
function Seg<T extends string>(props: {
  value: T; options: [T, string][]; onChange: (v: T) => void; disabled?: boolean;
}) {
  return (
    <div className={'seg' + (props.disabled ? ' disabled' : '')}>
      {props.options.map(([v, label]) => (
        <button key={v} className={props.value === v ? 'active' : ''}
                disabled={props.disabled}
                onClick={() => props.onChange(v)}>{label}</button>
      ))}
    </div>
  );
}

/// 盤の下に置くスコア帯。石数・思考時間・結果を 1 行にまとめる。
/// 「いまの盤面がどうなっているか」の話なので盤に近い下側へ置き、
/// 「これから何をするか」の操作は盤の上 (ControlBar) にまとめてある。
export function ScoreBar({ g }: { g: PanelProps['g'] }) {
  const v = g.view;
  const anyThink = g.thinkTotal.black > 0 || g.thinkTotal.white > 0;
  const result = v?.over
    ? (v.black === v.white ? '引き分け' : v.black > v.white ? '黒の勝ち' : '白の勝ち')
    : '';
  return (
    <div className="scorebar">
      <span className={'side' + (v && !v.over && v.player === 'black' ? ' turn' : '')}>
        <i className="disc b" />{v?.black ?? 2}
      </span>
      <div className="mid">
        {result && <span className="result">{result}</span>}
        {/* 直前の手を KUROOBI がどう見たか。指した結果の話なので、盤面の
            状況と一緒にこちらへ置く (思考の進行は盤の上の帯が受け持つ)。
            幅が足りなければ末尾から畳む。全文は title で読める */}
        {g.lastEval && (
          <span className="eval-inline" title={`KUROOBI の評価 ${g.lastEval}`}>
            <span className="lbl">KUROOBI の評価</span>{g.lastEval}
          </span>
        )}
        {anyThink && (
          <span className="times">
            <i className="disc b" />{fmtSecs(g.thinkTotal.black)}
            <span className="vs">思考時間</span>
            {fmtSecs(g.thinkTotal.white)}<i className="disc w" />
          </span>
        )}
      </div>
      <span className={'side' + (v && !v.over && v.player === 'white' ? ' turn' : '')}>
        {v?.white ?? 2}<i className="disc w" />
      </span>
    </div>
  );
}

/// 盤の上に置く操作帯。対局の開始・停止、新規対局、待った、評価値の
/// 表示切り替えと、KUROOBI がいま何をしているか (思考中の秒数・ノード数・
/// 速さ) を 1 行にまとめる。
///
/// 押すものと KUROOBI の状態を隣に置いたのは、どちらも「これから / いま
/// 何が起きるか」の話だから。盤面そのものの状況 (石数・結果) は下の
/// ScoreBar が受け持つ。
export function ControlBar({ g, onStart }: { g: PanelProps['g']; onStart: () => void }) {
  const v = g.view;
  const vs = g.mode === 'vs';
  return (
    <div className="controlbar">
      {vs && (
        <>
          {/* 主動作は 1 つだけ塗る。並んだボタンが全部同じ重さだと、
              どれを押せばいいのかが見た目から分からない */}
          {/* ラベルは幅が足りなくなると畳まれてアイコンだけになる。
              その状態でも何のボタンか分かるよう title を必ず付ける */}
          <button className={'hbtn ' + (g.playing ? 'stop' : 'go')}
                  disabled={!g.playing && (!!v?.over || g.thinking)}
                  title={g.playing ? '対局を停止する' : '対局を始める'}
                  onClick={onStart}>
            <Icon name={g.playing ? 'stop' : 'start'} size={15} />
            <span className="lbl">{g.playing ? '対局停止' : '対局開始'}</span>
          </button>
          <button className="hbtn" disabled={g.thinking} title="新規対局 (盤を初期配置に戻す)"
                  onClick={() => void g.newGame()}>
            <Icon name="newgame" size={15} /><span className="lbl">新規対局</span>
          </button>
          <button className="hbtn" disabled={g.thinking || !v || v.move_count === 0}
                  title="待った (一手戻す)"
                  onClick={() => void g.undo()}>
            <Icon name="undo" size={15} /><span className="lbl">待った</span>
          </button>
          <span className="hsep" />
        </>
      )}
      {/* 設定は Seg に揃えてあるが、ここは設定欄ではなく道具の並びなので
          押し込み式の 1 ボタンにする (オフ/オンの 2 枠は帯の中では重い) */}
      <button className={'hbtn toggle' + (g.autoHint ? ' on' : '')}
              aria-pressed={g.autoHint}
              title={g.autoHint ? '評価値を消す' : '盤に評価値を出す'}
              onClick={() => g.setAutoHint(!g.autoHint)}>
        <Icon name="hint" size={15} /><span className="lbl">評価値</span>
      </button>
      <EngineStatus g={g} />
    </div>
  );
}

/// 帯の右端: KUROOBI がいま何をしているか。思考中の秒数と、探索した
/// ノード数・その速さ。単位だけは英語のまま — 数量の単位で、訳すと
/// かえって読みにくい。
///
/// ここに置くのは**短くて桁の決まっているものだけ**にしてある。長さの
/// 読めない伝言を混ぜると、幅の取り合いで数字が押し出される
/// (盤の横幅は 400px 台まで狭くなりうる)。失敗などの報せは Toasts。
///
/// 定石から返した手はノードを訪れないので数字が出ない。0 を出すと「働いて
/// いない」ではなく「壊れている」に見えるため、その場合は消す。
function EngineStatus({ g }: { g: PanelProps['g'] }) {
  const stat: SearchStat | null = g.stat;
  const nodes = stat && stat.nodes > 0 ? stat.nodes : 0;
  const nps = stat && nodes > 0 && stat.secs > 0 ? nodes / stat.secs : 0;
  return (
    <div className="cb-stat">
      {g.thinking && <span className="thinking">思考中… {g.thinkSecs.toFixed(1)} 秒</span>}
      {nodes > 0 && <span className="num"><b>{fmtNodes(nodes)}</b> nodes</span>}
      {nps > 0 && <span className="num nps"><b>{fmtNodes(nps)}</b> nps</span>}
    </div>
  );
}

/// 報せ。**出るのは失敗と、押したのに進まない理由だけ**
/// (「棋譜が見つかりません」「GGS 対局中は分析を控えます」)。
///
/// 浮かせてあるのは、盤の下に行を置くと出入りのたびに帯の高さが動いて画面
/// 全体が跳ねるから。内容の並びに場所を取らせない。
///
/// ここに入れないもの:
/// - **作業が進んでいることの報せ** — その作業を出している場所が自分で持つ
///   (分析の進み具合なら評価値グラフの節)
/// - **押した本人が見れば分かること** (「棋譜を読み込みました」「対局を
///   停止しました」)。読む前に消えるうえ、報せとして何も足していない
/// - **エンジンの内部の符丁** (`stopped` など)。state.ts の INTERNAL で落とす
///
/// ただし**失敗は必ず出す**。無言で終わる経路を作ると、動かない理由が画面の
/// どこにも残らない。
export function Toasts({ items, onClose }: {
  items: Game['toasts']; onClose: (id: number) => void;
}) {
  if (!items.length) return null;
  return (
    <div className="toasts">
      {items.map((t) => (
        // 読み終わって邪魔なら押して消せる (待たせない)
        <button key={t.id} className="toast" onClick={() => onClose(t.id)}>
          {t.text}
        </button>
      ))}
    </div>
  );
}

export function Panel({ g, gvals, onSave, onLoad }: PanelProps) {
  const v = g.view;
  const vs = g.mode === 'vs';

  return (
    <div id="panel">
      <div className="panel-sec">
        {vs && (
          <div>
            <label className="field">KUROOBI の担当</label>
            <Seg value={g.side} onChange={g.setSide} options={[
              ['black', '黒'], ['white', '白'], ['both', '両方'], ['off', 'なし'],
            ]} />
          </div>
        )}
        {/* 選び方は GGS の設定と共通 (Strength)。同じ 3 つを決めるのに
            操作が違うと、片方で覚えたことがもう片方で通じない */}
        <Strength value={g.levels} onChange={(v) => {
          const i = LEVELS.findIndex(
            (lv) => lv.depth === v.depth && lv.solve === v.solve && lv.band === v.band);
          if (i >= 0) { g.setLevel(i); return; }
          g.setCustom(v);
          g.setLevel('custom');
        }} />
        <div>
          <label className="field">
            定石{!g.hasBook && (
              <span className="hint-inline"> — ファイルがありません (設定から指定できます)</span>
            )}
          </label>
          <Seg value={g.useBook ? 'on' : 'off'} disabled={!g.hasBook}
               onChange={(x) => g.setUseBook(x === 'on')}
               options={[['on', '使う'], ['off', '使わない']]} />
        </div>
      </div>

      {/* 操作・思考中・伝言は盤の上の帯へ、直前の手の評価は盤の下のスコア帯へ
          移した。右ペインに残すのは「あらかじめ決める設定」と棋譜だけ */}

      <div className="kifu-wrap">
        {/* 見出しと保存/読込だけが余白を持つ。表は端まで伸ばして列幅を稼ぐ */}
        <div className="row kifu-head" style={{ alignItems: 'center' }}>
          <label className="field" style={{ flex: 1, margin: 0 }}>
            棋譜 (評価は黒視点)
          </label>
          <button className="btn small" onClick={onSave}>保存</button>
          <button className="btn small" onClick={onLoad}>読込</button>
        </div>
        {v && (
          <Kifu moves={v.moves} cursor={v.cursor} info={g.moveSource} values={gvals}
                onJump={(n) => void g.jumpTo(n)} />
        )}
      </div>
    </div>
  );
}
