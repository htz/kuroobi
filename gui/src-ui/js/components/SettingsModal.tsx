// 設定 (歯車)。エンジンが使うファイルを選び直す。
import { useCallback, useState } from 'react';
import { api } from '../api';

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
  onClose: () => void;
  /** ファイルを変えたら呼ぶ (定石の有無が変わるため)。 */
  onChanged: () => void;
}

/// 開いているときだけ描く。呼ぶ側で出し分けることで、
/// 「閉じているのに中身を取りに行く」経路をなくしている。
export function SettingsModal({ initial, onClose, onChanged }: SettingsModalProps) {
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
        <div className="row actions">
          <button className="btn ghost" onClick={onClose}>閉じる</button>
        </div>
      </div>
    </div>
  );
}
