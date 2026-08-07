import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { api, emitApp, type KifuFrame, type ThreadsView } from './api';
import type { Prefs } from './prefs';
import { Modal, Overlay, Section, WindowBar } from './components/layout';
import { Button, Segmented, Select, TextField } from './components/primitives';
import { Icon } from './components/Icons';
import { Board } from './components/board';

/* 確認と入力。現行がブラウザの confirm() / prompt() を使っているところを
 * 置き換える。ブラウザのダイアログはデザインに乗らないうえ、Tauri の
 * WebView では見た目が OS 任せになる。 */

export function Confirm({ title, body, ok = 'OK', danger, onOk, onCancel }: {
  title: string; body?: React.ReactNode; ok?: string; danger?: boolean;
  onOk: () => void; onCancel: () => void;
}) {
  return (
    <Overlay onClose={onCancel}>
      <Modal title={title} body={body} actions={<>
        <Button onClick={onCancel}>やめる</Button>
        <Button variant={danger ? 'danger' : 'primary'} onClick={onOk}>{ok}</Button>
      </>} />
    </Overlay>
  );
}

/** 一覧から 1 つ選ぶ (チャットの「新しい相手」)。 */
export function PickOne({ title, body, options, ok = '開く', onOk, onCancel }: {
  title: string; body?: React.ReactNode; options: [string, string][];
  ok?: string; onOk: (v: string) => void; onCancel: () => void;
}) {
  const [v, setV] = useState(options[0]?.[0] ?? '');
  return (
    <Overlay onClose={onCancel}>
      <Modal title={title}
             body={<div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
               {body}
               <Select value={v} options={options} onChange={setV} />
             </div>}
             actions={<>
               <Button onClick={onCancel}>やめる</Button>
               <Button variant="primary" disabled={!v} onClick={() => onOk(v)}>{ok}</Button>
             </>} />
    </Overlay>
  );
}

/** 棋譜を貼って読み込む。ファイルから選ぶ経路も同じ箱に置く。 */
export function PasteKifu({ onLoad, onFile, onCancel }: {
  onLoad: (text: string) => void; onFile: () => void; onCancel: () => void;
}) {
  const [text, setText] = useState('');
  /* 貼ったものが読めるかを、読み込む前に見せる。
   *
   * GGF・着手列・盤面つきのどれでも受けるので、**読めたかどうかが押すまで
   * 分からない**のが困る。バックエンドに下読みだけさせる (対局の状態は
   * 動かさない)。打鍵のたびに投げないよう少し待ってから。 */
  const [peek, setPeek] = useState<{ frames: KifuFrame[]; err: string } | null>(null);
  useEffect(() => {
    const t = text.trim();
    let alive = true;
    if (!t) {
      // 空にしたときも遅らせて消す。effect の中で直に状態を書かない
      const clear = setTimeout(() => { if (alive) setPeek(null); }, 0);
      return () => { alive = false; clearTimeout(clear); };
    }
    const id = setTimeout(() => {
      void api.previewKifu(t)
        .then((frames) => { if (alive) setPeek({ frames, err: '' }); })
        .catch((e) => { if (alive) setPeek({ frames: [], err: '' + e }); });
    }, 250);
    return () => { alive = false; clearTimeout(id); };
  }, [text]);

  // frames の 1 枚目は初期局面。2 枚以上なければ手が 1 つも読めていない
  const ok = !!peek && !peek.err && peek.frames.length > 1;
  const last = ok ? peek!.frames[peek!.frames.length - 1] : null;
  return (
    <Overlay onClose={onCancel}>
      <div role="dialog" aria-modal style={{
        width: 460, borderRadius: 'var(--r-4)', background: 'var(--card)',
        border: '1px solid var(--border)', boxShadow: 'var(--sh-2)',
        padding: 'var(--sp-5)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-4)',
      }}>
        <div style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>棋譜を読み込む</div>
        <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.7 }}>
          GGF・f5d6… 形式・盤面つきのいずれでも読めます。
        </div>
        <textarea value={text} onChange={(e) => setText(e.target.value)}
          className="k-input"
          style={{
            height: 160, resize: 'none', padding: 'var(--sp-3)', borderRadius: 'var(--r-3)',
            background: 'var(--bg)', border: '1px solid var(--border)',
            fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', lineHeight: 1.6,
          }} />
        {/* 下読みの結果。読めなければ理由、読めれば手数と終局図 */}
        <div style={{ display: 'flex', gap: 'var(--sp-4)', alignItems: 'center', minHeight: 96 }}>
          <div style={{ width: 96, flex: 'none' }}>
            {last && <Board cells={last.cells as (0 | 1 | 2)[]} last={last.last} coords={false} grain={false} />}
          </div>
          <div style={{ fontSize: 'var(--fs-6)', color: peek?.err ? 'var(--bad)' : 'var(--sub)', lineHeight: 1.7 }}>
            {!peek && '貼り付けると、ここに読み取り結果が出ます。'}
            {peek?.err && peek.err}
            {last && <>
              {/* 初期局面ぶんの 1 枚を引いて手数にする */}
              {peek!.frames.length - 1} 手 ／ 黒 <b style={{ color: 'var(--text)' }}>{last.black}</b>
              {' '}白 <b style={{ color: 'var(--text)' }}>{last.white}</b>
            </>}
            {peek && !peek.err && !ok && '手が 1 つも読み取れません。'}
          </div>
        </div>

        <div style={{ display: 'flex', gap: 'var(--sp-2)', alignItems: 'center' }}>
          <Button onClick={onFile}>ファイルから…</Button>
          <span style={{ marginLeft: 'auto' }} />
          <Button onClick={onCancel}>やめる</Button>
          <Button variant="primary" disabled={!ok} onClick={() => onLoad(text)}>読み込む</Button>
        </div>
      </div>
    </Overlay>
  );
}

/* ---------------- 設定 (歯車) ----------------
 *
 * エンジンが使うファイル・ローカル探索のスレッド数・学習の取り込み。
 * GGS 用のエンジン設定は GGS の画面が持つ (エンジンが別なので設定も別)。
 *
 * 設計では独立ウィンドウだが、窓を増やすのは段階 6 の後。まずは暗幕の中に置く。
 */

// 画面に出す名前は resource_status が返すものと同じにする
// (食い違うと状態が引けず、パスも出なくなる)
// 並びは設計と同じ「NNUE → 線形評価 → 定石」。KUROOBI が主に読むものが上。
const KINDS: [string, string][] = [
  ['nnue', 'NNUE の重み'],
  ['weights', '線形評価の重み'],
  ['book', '定石'],
];

/* 足りないときは、無いことだけでなく**その先どうなるか**を言う。
 * 「ファイルがありません」だけだと、直さないと動かないのか、
 * 直さなくても済むのかが分からない。 */
const NOT_FOUND: Record<string, string> = {
  weights: '見つかりません。KUROOBI は評価できません',
  nnue: '見つかりません。線形評価だけで指します',
  book: '見つかりません。実戦から育つ book_learn.txt だけを使います',
};

/* 設定は主画面の脇役ではないので**別の窓**に出す (規則 79)。覆いだと後ろの
 * 盤が見えているのに触れず、開いている間ずっと主画面が止まって見える。
 *
 * 窓が別なら React の状態は共有されないので、
 *   - 見え方 (Prefs) は localStorage 経由。書けば storage が飛んで主画面が追う
 *   - ファイルとスレッドはバックエンドが持つので、変えたら報せだけ送る
 * という分担にしてある。学習の取り込みはドックに操作があるので置かない
 * (同じ設定を 2 か所に出さない — 規則 58)。 */
export function Settings({ prefs, setPref }: {
  prefs: Prefs; setPref: <K extends keyof Prefs>(k: K, v: Prefs[K]) => void;
}) {
  const [tab, setTab] = useState<'engine' | 'view'>('engine');
  const [reset, setReset] = useState(false);
  const [status, setStatus] = useState<[string, string, boolean][]>([]);
  const [th, setTh] = useState<ThreadsView | null>(null);

  const load = useCallback(async () => {
    try { setStatus(await api.resourceStatus()); } catch { /* エンジン未初期化 */ }
  }, []);

  // 開いた時点の状態を取る。閉じた後に届いても捨てる
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const [st, t] = await Promise.all([api.resourceStatus(), api.localThreads()]);
        if (!alive) return;
        setStatus(st);
        setTh(t);
      } catch { /* エンジン未初期化 */ }
    })();
    return () => { alive = false; };
  }, []);

  const byName = new Map(status.map(([n, p, ok]) => [n, { p, ok }]));
  const change = async (kind: string, path: string | null) => {
    await api.setResource(kind, path);
    await load();
    // 主画面は別の document なので自分では気付けない
    emitApp('resources-changed');
  };
  const setThreads = async (n: number | null) => {
    try {
      await api.setLocalThreads(n);
      setTh(await api.localThreads());
    } catch { /* 保存失敗はそのまま */ }
  };

  return (
    <div style={{ height: '100%', display: 'flex', flexDirection: 'column', background: 'var(--bg)' }}>
      <WindowBar title="設定" />
      <div style={{
        flex: 1, minHeight: 0, background: 'var(--card)',
        display: 'flex', flexDirection: 'column', overflow: 'hidden',
      }}>
        {/* 節を縦に積むと下まで巻かないと何があるか分からない。区分はタブで
            分ける (設計の 設定 と同じ形)。GGS は左メニューに行き先があるので
            ここには置かない — 同じ設定を 2 か所に出さない (規則 58) */}
        <div style={{
          flex: 'none', display: 'flex', justifyContent: 'center',
          padding: 'var(--sp-3) var(--sp-5)', borderBottom: '1px solid var(--border-weak)',
        }}>
          <Segmented value={tab} onChange={setTab} options={[
            { value: 'engine', label: 'エンジン' },
            { value: 'view', label: '表示' },
          ]} />
        </div>
        <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-5)' }}>

        {/* 見え方だけの設定。エンジンの動きには関わらないので、
            バックエンドに送らず localStorage に置く */}
        {tab === 'view' && <>
        <Section title="盤と配色">
          <Row2 label="テーマ">
            <Segmented value={prefs.theme} onChange={(v) => setPref('theme', v)} options={[
              { value: 'os', label: 'OS に従う' },
              { value: 'dark', label: 'ダーク' },
              { value: 'light', label: 'ライト' },
            ]} />
          </Row2>
          <Row2 label="盤の向き">
            <Segmented value={prefs.facing} onChange={(v) => setPref('facing', v)} options={[
              { value: 'black', label: '黒が下' },
              { value: 'white', label: '白が下' },
              { value: 'auto', label: '自分が下' },
            ]} />
          </Row2>
          <Row2 label="座標">
            <Segmented value={prefs.coords ? 'on' : 'off'} onChange={(v) => setPref('coords', v === 'on')}
                       options={[{ value: 'on', label: '出す' }, { value: 'off', label: '出さない' }]} />
          </Row2>
          <Row2 label="畳の織り目">
            <Segmented value={prefs.grain ? 'on' : 'off'} onChange={(v) => setPref('grain', v === 'on')}
                       options={[{ value: 'on', label: '出す' }, { value: 'off', label: '出さない' }]} />
          </Row2>
          <Row2 label="石返し">
            <Segmented value={String(prefs.flipMs)} onChange={(v) => setPref('flipMs', +v as Prefs['flipMs'])}
                       options={[{ value: '0', label: '動かさない' },
                                 { value: '120', label: '速い' },
                                 { value: '240', label: 'ゆっくり' }]} />
          </Row2>
        </Section>
        </>}

        {tab === 'engine' && <>
        <Section title="ファイル">
          {KINDS.map(([kind, title]) => {
            const info = byName.get(title);
            return (
              <div key={kind} style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                {/* パスは打ち込む欄に入れる。行の右端に押しやると、いま何を
                    読んでいるかとその直し方が別の場所に散る */}
                <Row2 label={title}>
                  <TextField value={info?.p ?? ''} placeholder="未指定" invalid={!info?.ok} />
                  <Button onClick={async () => {
                    const p = await api.pickResource(kind);
                    if (p) await change(kind, p);
                  }}>選択…</Button>
                </Row2>
                {/* 良し悪しは欄の直下に置く。欄の列に揃えるので左に見出しぶんを空ける */}
                <div style={{
                  marginLeft: 'calc(var(--w-label) + var(--sp-3))',
                  display: 'flex', alignItems: 'center', gap: 6,
                  fontSize: 'var(--fs-7)', color: info?.ok ? 'var(--ok)' : 'var(--bad)',
                }}>
                  <Icon name={info?.ok ? 'check' : 'alert'} size={12} />
                  {info?.ok ? '読み込み済み' : NOT_FOUND[kind]}
                </div>
              </div>
            );
          })}
        </Section>

        {th && (
          <Section title="ローカル探索のスレッド数">
            <Row2 label="スレッド">
              <Select width={140} value={th.set == null ? 'auto' : String(th.set)}
                      onChange={(v) => void setThreads(v === 'auto' ? null : +v)}
                      options={[['auto', `自動 (${th.auto})`],
                                ...Array.from({ length: th.auto * 2 }, (_, i) =>
                                  [String(i + 1), String(i + 1)] as [string, string])]} />
            </Row2>
            {/* 説明は操作の下。上に置くと、読まないと何を選ぶ場所か分からない
                欄に見える (欄そのものは「スレッド」で足りている) */}
            <p style={{ margin: 0, marginLeft: 'calc(var(--w-label) + var(--sp-3))',
                        fontSize: 'var(--fs-7)', color: 'var(--sub)', lineHeight: 1.8 }}>
              ローカル対局・検討・学習の取り込みが使う並列数です。自動 = コア数の半分 ({th.auto})。
              GGS 対局用は GGS の設定にあります (別々に動くので、両方が同時に動くと合計ぶんの CPU を使います)。
            </p>
          </Section>
        )}
        </>}

        </div>
        {/* OK ボタンは置かない (変更は即時に反映される)。それが分からないと
            「決定していないのでは」と不安になるので、下の帯で言い切る */}
        <div style={{
          flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
          padding: 'var(--sp-3) var(--sp-5)', borderTop: '1px solid var(--border-weak)',
        }}>
          {tab === 'engine' && (
            <Button variant="ghost" onClick={() => setReset(true)}>既定に戻す</Button>
          )}
          <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>
            変更は即時に反映されます
          </span>
        </div>
      </div>
      {reset && (
        <Confirm title="ファイルの指定を既定に戻しますか？"
                 body="選んだ重みと定石の場所を忘れ、既定の探し方に戻します。ファイルそのものは消えません。"
                 ok="戻す"
                 onCancel={() => setReset(false)}
                 onOk={() => {
                   setReset(false);
                   void (async () => {
                     for (const [kind] of KINDS) await api.setResource(kind, null);
                     await setThreads(null);
                     await load();
                     emitApp('resources-changed');
                   })();
                 }} />
      )}
    </div>
  );
}

/** 設定の 1 行。見出しの列を揃える。
 *  見出しは右揃え — 語の長さがまちまちなので、左揃えだと欄との間隔が
 *  行ごとに変わって、欄の列が揃っていても揃って見えない。 */
function Row2({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', minHeight: 'var(--h-field)' }}>
      <span style={{ width: 'var(--w-label)', flex: 'none', textAlign: 'right',
                     fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{label}</span>
      {children}
    </div>
  );
}
