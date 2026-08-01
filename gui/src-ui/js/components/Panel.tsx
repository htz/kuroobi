// 右の設定パネル。石数・思考時間・エンジンの設定・棋譜。
import { LEVELS } from '../state';
import type { Game } from '../state';
import { Kifu } from './Kifu';

const fmtSecs = (v: number): string =>
  v >= 60 ? `${Math.floor(v / 60)} 分 ${(v % 60).toFixed(0)} 秒` : `${v.toFixed(1)} 秒`;

export interface PanelProps {
  g: Game;
  onStart: () => void;
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

export function Panel({ g, onStart, onSave, onLoad }: PanelProps) {
  const v = g.view;
  const vs = g.mode === 'vs';
  const anyThink = g.thinkTotal.black > 0 || g.thinkTotal.white > 0;

  return (
    <div id="panel">
      <div className="card">
        <div className="score">
          <span className={'side' + (v && !v.over && v.player === 'black' ? ' turn' : '')}>
            <i className="disc b" />{v?.black ?? 2}
          </span>
          <span className="vs">—</span>
          <span className={'side' + (v && !v.over && v.player === 'white' ? ' turn' : '')}>
            {v?.white ?? 2}<i className="disc w" />
          </span>
        </div>
        {anyThink && (
          <div className="think-time">
            <span className="side"><i className="disc b" />{fmtSecs(g.thinkTotal.black)}</span>
            <span className="vs">思考時間</span>
            <span className="side">{fmtSecs(g.thinkTotal.white)}<i className="disc w" /></span>
          </div>
        )}
        <div id="result">
          {v?.over
            ? (v.black === v.white ? '引き分け'
              : (v.black > v.white ? '黒の勝ち' : '白の勝ち'))
            : ''}
        </div>
      </div>

      <div className="card">
        {vs && (
          <div>
            <label className="field">エンジンの担当</label>
            <Seg value={g.side} onChange={g.setSide} options={[
              ['black', '黒'], ['white', '白'], ['both', '両方'], ['off', 'なし'],
            ]} />
          </div>
        )}
        <div>
          <label className="field">強さ</label>
          <div className="selwrap">
            <select value={String(g.level)}
                    onChange={(e) => g.setLevel(
                      e.target.value === 'custom' ? 'custom' : +e.target.value)}>
              {LEVELS.map((lv, i) => (
                <option key={i} value={i}>
                  {`${lv.name} — 深さ${lv.depth} / 読切${lv.solve}${
                    lv.band ? ` / 選択読み+${lv.band}` : ''}`}
                </option>
              ))}
              <option value="custom">カスタム…</option>
            </select>
          </div>
        </div>
        <div>
          <label className="field">
            定石{!g.hasBook && (
              <span className="hint-inline"> — ファイルがありません (歯車から指定できます)</span>
            )}
          </label>
          <Seg value={g.useBook ? 'on' : 'off'} disabled={!g.hasBook}
               onChange={(x) => g.setUseBook(x === 'on')}
               options={[['on', '使う'], ['off', '使わない']]} />
        </div>
        {g.level === 'custom' && (
          <div className="row">
            <div>
              <label className="field">深さ</label>
              <div className="selwrap">
                <select value={g.custom.depth}
                        onChange={(e) => g.setCustom({ ...g.custom, depth: +e.target.value })}>
                  {Array.from({ length: 60 }, (_, i) => i + 1).map((n) => (
                    <option key={n} value={n}>{n}</option>))}
                </select>
              </div>
            </div>
            <div>
              <label className="field">読切</label>
              <div className="selwrap">
                <select value={g.custom.solve}
                        onChange={(e) => g.setCustom({ ...g.custom, solve: +e.target.value })}>
                  {Array.from({ length: 37 }, (_, i) => i).map((n) => (
                    <option key={n} value={n}>{n === 0 ? 'なし' : n}</option>))}
                </select>
              </div>
            </div>
            <div>
              <label className="field">選択読み</label>
              <div className="selwrap">
                <select value={g.custom.band}
                        onChange={(e) => g.setCustom({ ...g.custom, band: +e.target.value })}>
                  {Array.from({ length: 13 }, (_, i) => i).map((n) => (
                    <option key={n} value={n}>{n === 0 ? 'なし' : `+${n}`}</option>))}
                </select>
              </div>
            </div>
          </div>
        )}
      </div>

      <div className="card">
        {vs && (
          <button className={'btn' + (g.playing ? '' : ' primary')}
                  disabled={!g.playing && (!!v?.over || g.thinking)}
                  onClick={onStart}>
            {g.playing ? '■ 対局停止' : '▶ 対局開始'}
          </button>
        )}
        {vs && (
          <div className="row" id="game-btns">
            <button className="btn" disabled={g.thinking} onClick={() => void g.newGame()}>
              新規対局
            </button>
            <button className="btn" disabled={g.thinking || !v || v.move_count === 0}
                    onClick={() => void g.undo()}>
              待った
            </button>
          </div>
        )}
        <div>
          <label className="field">評価値を表示 (全合法手を自動採点)</label>
          <Seg value={g.autoHint ? 'on' : 'off'}
               onChange={(x) => g.setAutoHint(x === 'on')}
               options={[['off', 'オフ'], ['on', 'オン']]} />
        </div>
        <div id="status" className={g.spin ? 'spin' : ''}>{g.status}</div>
      </div>

      <div>
        <div className="row" style={{ alignItems: 'center', marginBottom: 4 }}>
          <label className="field" style={{ flex: 1, margin: 0 }}>
            棋譜 <span style={{ color: 'var(--gold)' }}>|</span>定石{' '}
            <span style={{ color: 'var(--accent)' }}>|</span>探索
          </label>
          <button className="btn small" onClick={onSave}>保存</button>
          <button className="btn small" onClick={onLoad}>読込</button>
        </div>
        {v && (
          <Kifu moves={v.moves} cursor={v.cursor} source={g.moveSource}
                onJump={(n) => void g.jumpTo(n)} />
        )}
      </div>
    </div>
  );
}
