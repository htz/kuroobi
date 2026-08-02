// 設定 (歯車)。エンジンが使うファイルを選び直す。
import { useCallback, useEffect, useState } from 'react';
import { api } from '../api';
import type { ThreadsView } from '../api';

const KINDS: [string, string][] = [
  ['dir', '置き場所 (フォルダ)'],
  ['weights', '線形評価の重み'],
  ['nnue', 'NNUE の重み'],
  ['book', '定石のファイル'],
];
// resource_status が返す名前 → 画面の種別
const BY_KIND: Record<string, string> = {
  weights: '線形評価の重み', nnue: 'NNUE の重み', book: '定石のファイル',
};

export interface SettingsModalProps {
  /** 開いた時点のファイルの状態。呼ぶ側が取ってから渡す。 */
  initial: [string, string, boolean][];
  /** 終局した対局を定石の学習に取り込むか (ローカル・GGS 共通)。 */
  learnOn: boolean;
  onLearn: (on: boolean) => void;
  onClose: () => void;
  /** ファイルを変えたら呼ぶ (定石の有無が変わるため)。 */
  onChanged: () => void;
}

/// 開いているときだけ描く。呼ぶ側で出し分けることで、
/// 「閉じているのに中身を取りに行く」経路をなくしている。
export function SettingsModal({ initial, learnOn, onLearn, onClose, onChanged }: SettingsModalProps) {
  const [status, setStatus] = useState(initial);

  // 変更したあとの取り直しだけ。開いた時点のものは呼ぶ側から受け取る。
  const load = useCallback(async () => {
    try { setStatus(await api.resourceStatus()); } catch { /* エンジン未初期化 */ }
  }, []);

  const byName = new Map(status.map(([n, p, ok]) => [n, { p, ok }]));

  const change = async (kind: string, path: string | null) => {
    await api.setResource(kind, path);
    await load();
    onChanged();
  };

  return (
    <div className="modal">
      <div className="card box wide">
        <h2>設定</h2>
        <div className="settings-section">
          <div className="settings-title">エンジンが使うファイル</div>
          <p className="hint">
            指定しなければ <code>weights/</code> を上へ辿って探します。
            個別に選ぶと、そのファイルだけを差し替えます。変更は次の思考から効きます。
          </p>
          {KINDS.map(([kind, title]) => {
            const info = kind === 'dir' ? null : byName.get(BY_KIND[kind]);
            return (
              <div className="file-row" key={kind}>
                <div className="file-head">
                  {title}
                  {info && (
                    <span className={'file-state ' + (info.ok ? 'ok' : 'ng')}>
                      {info.ok ? '見つかりました' : '見つかりません'}
                    </span>
                  )}
                </div>
                <div className="file-path">
                  {info ? info.p : '(未指定なら自動で探します)'}
                </div>
                <div className="row actions">
                  <button className="btn small" onClick={async () => {
                    const p = await api.pickResource(kind);
                    if (p) await change(kind, p);
                  }}>選ぶ…</button>
                  <button className="btn small ghost"
                          onClick={() => void change(kind, null)}>既定に戻す</button>
                </div>
              </div>
            );
          })}
        </div>
        <ThreadsSection />
        <div className="settings-section">
          <div className="settings-title">学習</div>
          <p className="hint">
            終局した対局 (ローカル対局と GGS の両方) を定石の学習に取り込みます。
            負けた展開は次から自然に避けられるようになります。学習分は定石とは
            別のファイル (book_learn.txt) に貯まります。
          </p>
          <div className="seg" style={{ alignSelf: 'flex-start' }}>
            <button className={learnOn ? 'active' : ''}
                    onClick={() => onLearn(true)}>取り込む</button>
            <button className={learnOn ? '' : 'active'}
                    onClick={() => onLearn(false)}>取り込まない</button>
          </div>
        </div>
        <div className="row actions">
          <button className="btn ghost" onClick={onClose}>閉じる</button>
        </div>
      </div>
    </div>
  );
}

/// ローカル探索のスレッド数。GGS の対局用は GGS 画面のエンジン設定にある
/// (エンジンが別なので設定も別)。値は resources.conf に保存される。
function ThreadsSection() {
  const [th, setTh] = useState<ThreadsView | null>(null);

  useEffect(() => {
    void api.localThreads().then(setTh).catch(() => {});
  }, []);

  const set = async (n: number | null) => {
    try {
      await api.setLocalThreads(n);
      setTh(await api.localThreads());
    } catch { /* 保存失敗はそのまま */ }
  };

  if (!th) return null;
  const options = [1, 2, 4, 6, 8, 12, 16].filter((n) => n <= th.auto * 2);
  return (
    <div className="settings-section">
      <div className="settings-title">ローカル探索のスレッド数</div>
      <p className="hint">
        ローカル対局・検討・学習の取り込みが使う並列数です。自動 = コア数の半分
        ({th.auto})。GGS 対局用は GGS 画面の「エンジン設定」にあります (エンジンが
        別々なので、両方が同時に動くと合計ぶんの CPU を使います)。
      </p>
      <div className="seg" style={{ alignSelf: 'flex-start' }}>
        <button className={th.set == null ? 'active' : ''}
                onClick={() => void set(null)}>自動</button>
        {options.map((n) => (
          <button key={n} className={th.set === n ? 'active' : ''}
                  onClick={() => void set(n)}>{n}</button>
        ))}
      </div>
    </div>
  );
}
