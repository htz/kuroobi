import { useCallback, useEffect, useState } from 'react';
import { api, type ThreadsView } from './api';
import { Modal, Overlay, Section } from './components/layout';
import { Button, Segmented, Select } from './components/primitives';
import { Icon, IconButton } from './components/Icons';

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
        <div style={{ display: 'flex', gap: 'var(--sp-2)', alignItems: 'center' }}>
          <Button onClick={onFile}>ファイルから…</Button>
          <span style={{ marginLeft: 'auto' }} />
          <Button onClick={onCancel}>やめる</Button>
          <Button variant="primary" disabled={!text.trim()} onClick={() => onLoad(text)}>読み込む</Button>
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
const KINDS: [string, string][] = [
  ['weights', '線形評価の重み'],
  ['nnue', 'NNUE の重み'],
  ['book', '定石'],
];

export function Settings({ learnOn, onLearn, onChanged, onClose }: {
  learnOn: boolean; onLearn: (on: boolean) => void; onChanged: () => void; onClose: () => void;
}) {
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
    onChanged();
  };
  const setThreads = async (n: number | null) => {
    try {
      await api.setLocalThreads(n);
      setTh(await api.localThreads());
    } catch { /* 保存失敗はそのまま */ }
  };

  return (
    <Overlay onClose={onClose}>
      <div role="dialog" aria-modal className="k-scroll" style={{
        width: 560, maxHeight: '80vh', borderRadius: 'var(--r-4)', background: 'var(--card)',
        border: '1px solid var(--border)', boxShadow: 'var(--sh-2)', padding: 'var(--sp-5)',
      }}>
        <div style={{ display: 'flex', alignItems: 'center', marginBottom: 'var(--sp-4)' }}>
          <span style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>設定</span>
          <span style={{ marginLeft: 'auto' }}><IconButton name="close" label="閉じる" onClick={onClose} /></span>
        </div>

        <Section title="KUROOBI が使うファイル">
          {KINDS.map(([kind, title]) => {
            const info = byName.get(title);
            return (
              <div key={kind} style={{
                display: 'flex', flexDirection: 'column', gap: 4,
                padding: '8px 0', borderBottom: '1px solid var(--border-weak)',
              }}>
                <span style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 'var(--fs-5)' }}>
                  {title}
                  {/* 使えているときは印だけ。足りないときだけ言葉で伝える */}
                  <Icon name={info?.ok ? 'check' : 'alert'} size={13} />
                  {!info?.ok && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--bad)' }}>ファイルがありません</span>}
                  <span style={{ marginLeft: 'auto', display: 'flex', gap: 'var(--sp-2)' }}>
                    <Button size="chip" onClick={async () => {
                      const p = await api.pickResource(kind);
                      if (p) await change(kind, p);
                    }}>選ぶ…</Button>
                    <Button size="chip" variant="ghost"
                            onClick={() => void change(kind, null)}>既定に戻す</Button>
                  </span>
                </span>
                <span style={{
                  fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-7)', color: 'var(--sub)',
                  overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                }}>{info?.p ?? '—'}</span>
              </div>
            );
          })}
        </Section>

        {th && (
          <Section title="ローカル探索のスレッド数">
            <p style={{ margin: 0, fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8 }}>
              ローカル対局・検討・学習の取り込みが使う並列数です。自動 = コア数の半分 ({th.auto})。
              GGS 対局用は GGS の設定にあります (別々に動くので、両方が同時に動くと合計ぶんの CPU を使います)。
            </p>
            <div>
              <Select width={140} value={th.set == null ? 'auto' : String(th.set)}
                      onChange={(v) => void setThreads(v === 'auto' ? null : +v)}
                      options={[['auto', `自動 (${th.auto})`],
                                ...Array.from({ length: th.auto * 2 }, (_, i) =>
                                  [String(i + 1), String(i + 1)] as [string, string])]} />
            </div>
          </Section>
        )}

        <Section title="学習">
          <p style={{ margin: 0, fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8 }}>
            終局した対局 (ローカル対局と GGS の両方) を定石の学習に取り込みます。
            負けた展開は次から自然に避けられるようになります。
            学習分は定石とは別のファイル (book_learn.txt) に貯まります。
          </p>
          <div>
            <Segmented value={learnOn ? 'on' : 'off'} onChange={(v) => onLearn(v === 'on')}
                       options={[{ value: 'on', label: 'する' }, { value: 'off', label: 'しない' }]} />
          </div>
        </Section>
      </div>
    </Overlay>
  );
}
