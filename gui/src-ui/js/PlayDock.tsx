import { Button, Progress, Segmented, Toggle } from './components/primitives';
import { Busy, Dock, KeyValue, Note, Section } from './components/layout';
import { KifuTable } from './components/data';
import { Strength } from './components/strength';
import { api } from './api';
import type { ActivityView } from './api';
import type { Prefs } from './prefs';
import type { BookBrowse } from './BookScreen';
import type { NavId } from './components/ggs';
import type { Move } from './components/data';
import { LEVELS } from './state';

/* 対局・検討の右ドック。**`App` の中に 117 行あった**ものを切り出した。
 * 3 枚のタブ (棋譜 / 強さ / 学習) はどれも「いま何が起きているか」を
 * 出すだけで、画面の骨格とは関わらない。
 *
 * 棋譜のときだけ丸ごとスクロールさせない — 表が列の見出しを固定して
 * 行だけを流す作りになっている (操作も上に残す)。 */
export function PlayDock({
  g, book, cpu, prefs, tab, onTab, open, onNav, onBookTab,
  onPaste, onLoadFile, study, moves, ggfNames,
}: {
  g: ReturnType<typeof import('./state').useGame>;
  book: BookBrowse;
  cpu: ActivityView | null;
  prefs: Prefs;
  tab: string;
  onTab: (t: string) => void;
  open: boolean;
  onNav: (id: NavId) => void;
  onBookTab: (t: string) => void;
  onPaste: () => void;
  onLoadFile: () => void;
  study: boolean;
  /** 棋譜表の行。`App` が控えから作る */
  moves: Move[];
  /** 保存のファイル名を作る (担当の色を含む)。`App` が持つ */
  ggfNames: () => [string, string];
}) {
  return (
    <Dock tabs={['棋譜', '強さ', '学習']} active={tab} onTab={onTab} open={open}
          scroll={tab !== '棋譜'}>
      {tab === '棋譜' && (
        <>
          <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
            <KifuTable moves={moves} current={g.view?.cursor} decimals={prefs.decimals}
                       onSelect={(n) => void g.jumpTo(n)} />
          </div>
          {/* 棋譜の出し入れは列の**下**。上に置くと、表を見に来ただけの
              ときにも操作が視線の先頭に来る。行は下へ伸びるので、
              出来上がった棋譜を渡す操作は行の終わりにあるほうが近い */}
          <div style={{
            flex: 'none', display: 'flex', gap: 'var(--sp-2)',
            padding: 'var(--sp-2) var(--sp-3)', borderTop: '1px solid var(--border-weak)',
          }}>
            {/* 検討ではツールバーの「棋譜を読み込む」が同じ覆いを開くので
                出さない。同じ操作を 1 画面に 2 つ置かない (規則 58)。
                対局では絵 (§1) どおりここに 3 つ並ぶ */}
            {!study && <Button title="⌘O" onClick={() => onPaste()}>貼り付け</Button>}
            <Button onClick={() => void onLoadFile()}>読込</Button>
            {/* .ggf で保存すると、どちらがどの色か・結果・開始局面まで入る。
                **1 手も無いときは押せなくする。**押すと「棋譜が空です」で
                返ってくるだけで、理由はすぐ上の表が「まだ手がありません」と
                言っている (規則 61 — 直し方は操作のそばに) */}
            <Button title="⌘S" disabled={!moves.length}
                    onClick={() => void api.saveKifu(...ggfNames()).catch((e: unknown) => g.say('' + e))}>
              保存
            </Button>
          </div>
        </>
      )}
      {tab === '強さ' && (
        // 検討では同じ 3 つが解析の深さになる。値も操作も同じなので節の名前だけ変える
        <Section title={study ? '解析の設定' : '強さ'}>
          {/* 選び方は GGS の設定と共通 (Strength)。同じ 3 つを決めるのに
              操作が違うと、片方で覚えたことがもう片方で通じない */}
          <Strength value={g.levels} onChange={(v) => {
            const i = LEVELS.findIndex((l) => l.depth === v.depth && l.solve === v.solve && l.band === v.band);
            if (i >= 0) { g.setLevel(i); return; }
            g.setCustom(v);
            g.setLevel('custom');
          }} />
          {/* 名前と「なぜ押せないか」を 1 行に、選ぶものはその下に全幅で。
              横に並べると 290px の枠では選択肢が中身の幅まで縮み、直し方の
              文言だけが釦の下に離れて置かれて、対応が読めなくなる (規則 61) */}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
            <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-2)' }}>
              <span style={{ fontSize: 'var(--fs-5)' }}>定石</span>
              {!g.hasBook && (
                <span style={{ fontSize: 'var(--fs-6)', color: 'var(--gold)' }}>
                  — ファイルがありません (設定から指定できます)
                </span>
              )}
            </div>
            <Segmented fill value={g.useBook ? 'on' : 'off'} disabled={!g.hasBook}
                       onChange={(x) => g.setUseBook(x === 'on')}
                       options={[{ value: 'on', label: '使う' }, { value: 'off', label: '使わない' }]} />
          </div>
        </Section>
      )}
      {tab === '学習' && (
        <>
          <Section title="定石への書き戻し">
            <Toggle checked={g.learnOn} onChange={g.setLearnOn} label="終局した対局を取り込む" />
            <Note>
              勝敗にかかわらず取り込み、終局の石差を根まで書き戻します。同じ負け方をなぞらなくなります。
            </Note>
          </Section>
          {/* 走っている間だけ枠を持つ (規則 13 の「箱を入れ子にしない」の
              例外は進行中のジョブだけ)。**「一時停止」は置かない** —
              止める口がエンジン側に無く、譲りは対局・検討が動くと自動で
              かかる。押せない釦を描くより、譲っていることを言う */}
          {/* 走っている間だけ出る枠。**節の見出しは付けない** — 設計 §4 は
              見出しの無い枠を 書き戻しの節と 学習した定石の節の間に挟んでいる。
              進行中のジョブだけが枠を持つ (規則 13 の「箱を入れ子にしない」の
              唯一の例外)。**「一時停止」は置かない** — 止める口がエンジン側に
              無く、譲りは対局・検討が動くと自動でかかる */}
          {cpu?.learn && (
            <div style={{
              border: '1px solid var(--border)', borderRadius: 'var(--r-2)',
              margin: '0 var(--sp-3)',
              padding: 'var(--sp-3)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
            }}>
              <div style={{ display: 'flex', alignItems: 'center', fontSize: 'var(--fs-5)' }}>
                <Busy>取り込み中</Busy>
                {/* 取り込みは 1 局面ずつ進む。桁が動くと「取り込み中」まで揺れる */}
                <span style={{
                  marginLeft: 'auto', fontSize: 'var(--fs-6)', color: 'var(--sub)',
                  fontVariantNumeric: 'tabular-nums',
                }}>
                  局面 {cpu.learn[0].toLocaleString()} / {cpu.learn[1].toLocaleString()}
                </span>
              </div>
              <Progress value={cpu.learn[1] > 0 ? cpu.learn[0] / cpu.learn[1] : 0} />
              {/* 譲っている間だけ下の行に出す。設計 §4 もこの位置
                  (絵は左に対局名、右に「譲り中」)。対局名を出す口は
                  エンジン側に無い */}
              {cpu.learn_paused && (
                <div style={{ display: 'flex', alignItems: 'center', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                  <span>対局・検討が動いている間は譲ります</span>
                  <span style={{ marginLeft: 'auto' }}>譲り中</span>
                </div>
              )}
            </div>
          )}
          {/* 明細は「定石」の画面の 2 枚目へ移した。同じ一覧をドックにも
              出すと、どちらが本物か分からなくなる (規則 58 と同じ話) */}
          <Section title="学習した定石">
            <KeyValue big label="登録局面" value={book.node?.size} />
            <KeyValue big label="うち学習" value={book.node?.learned_size} />
            <Button onClick={() => { onNav('book'); onBookTab('学習ログ'); }}>学習ログを見る</Button>
          </Section>
        </>
      )}
    </Dock>
  );
}
