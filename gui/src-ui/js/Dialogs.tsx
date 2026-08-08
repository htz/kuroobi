import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { api, emitApp, type KifuFrame, type ThreadsView } from './api';
import { TATAMI, type Prefs, type Theme } from './prefs';
import { Modal, Overlay, Section } from './components/layout';
import { Button, Segmented, Select, TextField } from './components/primitives';
import { Icon } from './components/Icons';
import { Board } from './components/board';
import { GgsSettings } from './GgsScreens';
import type { GgsSnapshot } from './types';

/* 確認と入力。現行がブラウザの confirm() / prompt() を使っているところを
 * 置き換える。ブラウザのダイアログはデザインに乗らないうえ、Tauri の
 * WebView では見た目が OS 任せになる。 */

export function Confirm({ title, body, ok = 'OK', danger, onOk, onCancel }: {
  title: string; body?: React.ReactNode; ok?: string; danger?: boolean;
  onOk: () => void; onCancel: () => void;
}) {
  return (
    <Overlay onClose={onCancel}>
      <Modal title={title} body={body} onClose={onCancel} actions={<>
        <span style={{ marginLeft: 'auto' }} />
        <Button size="field" onClick={onCancel}>やめる</Button>
        <Button size="field" variant={danger ? 'danger' : 'primary'} onClick={onOk}>{ok}</Button>
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
      <Modal title={title} onClose={onCancel}
             body={<div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
               {body}
               <Select value={v} options={options} onChange={setV} />
             </div>}
             actions={<>
               <span style={{ marginLeft: 'auto' }} />
               <Button size="field" onClick={onCancel}>やめる</Button>
               <Button size="field" variant="primary" disabled={!v} onClick={() => onOk(v)}>{ok}</Button>
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
      <Modal title="棋譜を読み込む" width="var(--w-modal-wide)" onClose={onCancel}
             sub="GGF・f5d6… 形式・盤面つきのいずれでも読めます。"
             actions={<>
               <Button size="field" onClick={onFile}>ファイルから…</Button>
               <span style={{ marginLeft: 'auto' }} />
               {/* 絵は「キャンセル」だが、実装は 3 か所とも「やめる」で
                   揃えてある (画面の文言は日本語で統一する)。要 push */}
               <Button size="field" onClick={onCancel}>やめる</Button>
               <Button size="field" variant="primary" disabled={!ok}
                       onClick={() => onLoad(text)}>読み込む</Button>
             </>}>
        <textarea value={text} onChange={(e) => setText(e.target.value)}
          className="k-input"
          style={{
            height: 110, resize: 'none', padding: 'var(--sp-3)', borderRadius: 'var(--r-3)',
            background: 'var(--bg)', border: '1px solid var(--border)',
            fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', lineHeight: 1.6,
          }} />
        {/* 下読みの結果。読めなければ理由、読めれば手数と終局図。
            **絵には無い** — 読み込んでから間違いに気付くのを避けるために
            足してある (絵にも入れてもらう。要 push) */}
        <div style={{ display: 'flex', gap: 'var(--sp-4)', alignItems: 'center', minHeight: 96 }}>
          <div style={{ width: 96, height: 96, flex: 'none' }}>
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
      </Modal>
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
/* 並びは設計と同じ「NNUE → 線形評価 → 定石」。KUROOBI が主に読むものが上。
 * 3 つ目は**画面に出す名前**で、設計の絵の文言に合わせてある。
 * 2 つ目はバックエンドが返す名前なので、勝手に変えると状態が引けなくなる。 */
const KINDS: [string, string, string][] = [
  ['nnue', 'NNUE の重み', 'NNUE 重み'],
  ['weights', '線形評価の重み', '線形評価'],
  ['book', '定石', '定石'],
];

/** ファイルの大きさ。桁が動くと読みにくいので、MB は小数 1 桁で止める。 */
function fmtSize(n: number): string {
  if (n <= 0) return '';
  if (n < 1024) return n + ' B';
  if (n < 1024 * 1024) return Math.round(n / 1024) + ' KB';
  return (n / 1024 / 1024).toFixed(1) + ' MB';
}

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
export function Settings({ prefs, setPref, ggs, onClose }: {
  prefs: Prefs; setPref: <K extends keyof Prefs>(k: K, v: Prefs[K]) => void;
  /** GGS のスナップショット。GGS タブの中身に使う (未接続なら null)。 */
  ggs?: GgsSnapshot | null;
  /** 閉じる (覆いの足の釦)。 */
  onClose?: () => void;
}) {
  const [tab, setTab] = useState<'engine' | 'view' | 'ggs'>('engine');
  /* 撮るためだけの入口。`KUROOBI_AUTOPLAY=settings:ggs` のようにタブを
     指定できる。タブはクリックでしか切り替えられず、確認が人手頼みだった。
     **`settings:view:light` のように 3 つ目を渡すとテーマを実際に変える** —
     押さずにテーマの切り替わりを撮るため */
  useEffect(() => {
    void api.autoplay().then((v) => {
      const [, t, arg] = (v ?? '').split(':');
      if (t === 'engine' || t === 'view' || t === 'ggs') setTab(t);
      if (arg === 'light' || arg === 'dark' || arg === 'os') {
        // 撮る側が「変える前」を押さえられるよう、少し待ってから変える
        window.setTimeout(() => setPref('theme', arg), 5000);
      }
    }).catch(() => { /* Tauri 外では効かないだけ */ });
    // setPref は窓が生きている間ずっと同じものなので、1 度だけでよい
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [reset, setReset] = useState(false);
  const [status, setStatus] = useState<[string, string, boolean, number, string][]>([]);
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

  const byName = new Map(status.map(([n, p, ok, size, kind]) => [n, { p, ok, size, kind }]));
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
    <Modal title="設定" width="560px" onClose={onClose} scroll
           band={<>
      {/* **タブの帯だけが明るい面 (--card)。** 覆いの頭は --panel、中身は
                   --bg で、間に挟まる帯が浮いて見える形 — 設計の絵がそうなっている。
                   逆にすると (帯が暗く中身が明るい) タブが頭と地続きになり、中身の
                   面だけが浮いた別の箱に見える。高さは --h-bar (44px)。

                   **タブは Segmented ではない。** 絵は囲みの枠を持たず、字を並べて
                   選ばれているものだけを塗る形。Segmented の囲みは「並んだ選択肢の
                   1 つ」を示す部品で、画面を切り替えるタブとは役目が違う
                   (規則 40 は選択肢の列の話)。 */}
               <div style={{
                 flex: 'none', height: 'var(--h-bar)', display: 'flex',
                 alignItems: 'center', justifyContent: 'center', gap: 'var(--sp-1)',
                          background: 'var(--card)', borderBottom: '1px solid var(--border)',
               }}>
                 {TABS.map(([v, label]) => {
                   const on = tab === v;
                   return (
                     <button key={v} type="button" className={'k-press' + (on ? ' k-on' : '')}
                             onClick={() => setTab(v)} aria-pressed={on}
                             style={{
                               height: 'var(--h-ctrl)', padding: '0 14px', border: 0,
                               borderRadius: 'var(--r-2)', fontSize: 'var(--fs-5)',
                               background: on ? 'var(--accent-dim)' : 'transparent',
                               color: on ? 'var(--on-accent)' : 'var(--sub)',
                               fontWeight: on ? 600 : 400,
                             }}>{label}</button>
                   );
                 })}
               </div>
           </>}>
      <div className="k-settings" style={{ display: 'flex', flexDirection: 'column' }}>

        {/* 見え方だけの設定。エンジンの動きには関わらないので、
            バックエンドに送らず localStorage に置く */}
        {tab === 'view' && <>
        <Section title="テーマ">
          {/* 設計は見本つきの札 3 枚。**配色は言葉より見たほうが早い** —
              「OS に従う」がどちらになるかも、札を見れば分かる */}
          <div style={{ display: 'flex', gap: 10 }}>
            {THEMES.map(([v, label]) => {
              const on = prefs.theme === v;
              return (
                <button key={v} type="button" className="k-press" onClick={() => setPref('theme', v)}
                  aria-pressed={on}
                  style={{
                    flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column',
                    gap: 'var(--sp-2)', padding: 'var(--sp-2)', borderRadius: 'var(--r-3)',
                    background: on ? 'var(--panel)' : 'transparent', fontSize: 'var(--fs-6)',
                    border: '1px solid ' + (on ? 'var(--accent)' : 'var(--border)'),
                    color: on ? 'var(--text)' : 'var(--sub)', fontWeight: 400,
                  }}>
                  <ThemeSwatch kind={v} />
                  {label}
                </button>
              );
            })}
          </div>
        </Section>
        <Section title="盤">
          <Row2 label="畳の色">
            {/* 設計は 4 色の見本。色の選択に文字を使うと、選んでから盤を
                見に行く往復が要る */}
            <span style={{ display: 'flex', gap: 'var(--sp-2)' }}>
              {TATAMI.map((t, i) => {
                const on = prefs.tatami === i;
                return (
                  <button key={t.label} type="button" className="k-press"
                          onClick={() => setPref('tatami', i as Prefs['tatami'])}
                          title={t.label} aria-label={t.label} aria-pressed={on}
                          style={{
                            width: 28, height: 28, borderRadius: 'var(--r-2)', padding: 0,
                            border: 0, background: t.board,
                            // 選ばれているものは外に 2px の輪。地が濃い色なので
                            // 枠を内側に取ると色が痩せて見える
                            boxShadow: on
                              ? '0 0 0 2px var(--accent), inset 0 0 0 1px var(--border)'
                              : 'inset 0 0 0 1px var(--border)',
                          }} />
                );
              })}
            </span>
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
        <Section title="数値">
          {/* 設計の 単位 / 視点 は置かない。単位は石差しか無く、視点は
              「盤の向き」と同じもの。**選べない行を置くと、触れるのに
              変わらない場所が増えるだけ** */}
          <Row2 label="小数">
            <Segmented value={String(prefs.decimals)}
                       onChange={(v) => setPref('decimals', +v as Prefs['decimals'])}
                       options={[{ value: '0', label: '0' },
                                 { value: '1', label: '1' },
                                 { value: '2', label: '2' }]} />
          </Row2>
        </Section>
        </>}

        {/* 設計どおり GGS の設定もこの窓に入れる。左メニューの行き先は
            落とした — 同じ設定へ行く道が 2 つあると、どちらが本物か
            分からなくなる (規則 58) */}
        {/* **未接続でもスナップショットは返る** (conn が "disconnected" の
            既定値)。**繋がないと丸ごと出さない作りだったが、それは行き過ぎ
            だった** — 強さ・持ち時間・定石・ふるまいは手元の設定で、
            `ggs.rs` の未接続の待ち受け (1015〜1055 行) がちゃんと受け取る。
            繋いでからでないと GGS 用の強さを決められないのはおかしい。
            サーバーに置いてある「申し込みの扱い」と「接続」だけを
            `GgsSettings` の中で畳む */}
        {tab === 'ggs' && (ggs
          ? <GgsSettings snap={ggs} />
          : (
            <Section title="GGS">
              <p style={{ margin: 0, maxWidth: 'var(--w-text)', fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8 }}>
                GGS の設定を読み込めていません。申し込みの扱いなどは
                サーバー側に残る設定なので、繋いでから読み書きします。
              </p>
            </Section>
          )
        )}


        {tab === 'engine' && <>
        <Section title="ファイル">
          {KINDS.map(([kind, title, label]) => {
            const info = byName.get(title);
            return (
              <div key={kind} style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                {/* パスは打ち込む欄に入れる。行の右端に押しやると、いま何を
                    読んでいるかとその直し方が別の場所に散る */}
                <Row2 label={label}>
                  <TextField value={info?.p ?? ''} placeholder="未指定" invalid={!info?.ok} />
                  <Button size="field" onClick={async () => {
                    const p = await api.pickResource(kind);
                    if (p) await change(kind, p);
                  }}>選択…</Button>
                </Row2>
                {/* 良し悪しは欄の直下、**欄の列に揃える** (設計も 96px 空けている) */}
                <div style={{
                  marginLeft: 'calc(var(--w-label) + var(--sp-3))',
                  display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                  fontSize: 'var(--fs-6)', color: info?.ok ? 'var(--ok)' : 'var(--bad)',
                }}>
                  <Icon name={info?.ok ? 'check' : 'alert'} size={12} />
                  {info?.ok
                    ? ['読み込み済み', fmtSize(info.size), info.kind].filter(Boolean).join(' · ')
                    : NOT_FOUND[kind]}
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
            {/* 説明は操作の下、節の幅いっぱい (設計の絵と同じ)。欄の列に
                字下げすると、欄の補足なのか節の説明なのかが曖昧になる */}
            <p style={{ margin: 0, maxWidth: 'var(--w-text)', fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8 }}>
              ローカル対局・検討・学習の取り込みが使う並列数です。自動 = コア数の半分 ({th.auto})。
              GGS 対局用は「GGS」タブにあります (別々に動くので、両方が同時に動くと合計ぶんの CPU を使います)。
            </p>
          </Section>
        )}
        </>}

        {/* OK ボタンは置かない (変更は即時に反映される)。それが分からないと
            「決定していないのでは」と不安になるので言い切る。
            **帯ではなく中身の最後の行**にする — 設計も上に罫を引いた行。

            **中身があるタブでだけ出す。** 「既定に戻す」はエンジンのタブの
            ものなので、表示 と GGS では罫だけが引かれて下に何も無い行に
            なっていた。空の罫は「何か足りない」としか読めない (規則 13 —
            節は見出しと罫で区切るもので、飾りに使わない) */}
        {tab === 'engine' && (
          <div style={{
            display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
            margin: '0 var(--sp-3)', paddingTop: 'var(--sp-4)',
            borderTop: '1px solid var(--border-weak)',
          }}>
            <Button size="field" onClick={() => setReset(true)}>既定に戻す</Button>
          </div>
        )}
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
    </Modal>
  );
}

/* 設計の 4 枚目「詳細」は落とした。絵に中身が描かれておらず、仕様にも
 * 該当するものが無い。**空のタブを置くと、押した人が探し物をして戻ってくる。** */
const TABS: ['engine' | 'view' | 'ggs', string][] = [
  ['engine', 'エンジン'], ['view', '表示'], ['ggs', 'GGS'],
];

const THEMES: [Theme, string][] = [
  ['os', 'システムに合わせる'], ['dark', 'ダーク'], ['light', 'ライト'],
];

/** テーマの見本。地の色だけを出す — 盤や文字まで描くと札が小さすぎて潰れる。
 *  「システムに合わせる」は 2 つが斜めに割れている絵にする。 */
function ThemeSwatch({ kind }: { kind: Theme }) {
  /* **ここだけリテラルの色を書く (規則 1 の例外)。** 見本は「いま選んで
     いないほうのテーマの地の色」を見せるものなので、`var(--bg)` では
     いま効いているテーマの値になり、3 枚の札が全部同じ色になる。
     値は tokens.css の `--bg` (ダーク / ライト) と揃えること。 */
  const dark = '#16191d', light = '#faf8f3';
  return (
    <span style={{
      display: 'block', height: 38, borderRadius: 'var(--r-1)', width: '100%',
      // ダークは地と同じ色なので、内側に 1px 入れないと札の中で溶ける
      boxShadow: kind === 'dark' ? 'inset 0 0 0 1px var(--border)' : undefined,
      background: kind === 'dark' ? dark : kind === 'light' ? light
        : `linear-gradient(135deg, ${dark} 50%, ${light} 50%)`,
    }} />
  );
}

/** 設定の 1 行。見出しの列を揃える。
 *  見出しは右揃え — 語の長さがまちまちなので、左揃えだと欄との間隔が
 *  行ごとに変わって、欄の列が揃っていても揃って見えない。 */
function Row2({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', minHeight: 'var(--h-field)' }}>
      <span style={{ width: 'var(--w-label)', flex: 'none', textAlign: 'right',
                     fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>{label}</span>
      {children}
    </div>
  );
}
