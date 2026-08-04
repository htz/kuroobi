// 設定 (歯車)。エンジンが使うファイルを選び直す。
import { useCallback, useEffect, useState } from 'react';
import { api } from '../api';
import type { ThreadsView } from '../api';
import { Modal } from './Modal';
import { Icon } from './Icons';

// 画面に出す名前は resource_status が返すものと同じにする
// (食い違うと状態が引けず、パスも出なくなる)
const KINDS: [string, string][] = [
  ['weights', '線形評価の重み'],
  ['nnue', 'NNUE の重み'],
  ['book', '定石'],
];

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
    <Modal title="設定" wide onClose={onClose}
           actions={<><span className="spacer" />
                      <button className="btn" onClick={onClose}>閉じる</button></>}>
        <div className="settings-section">
          <div className="settings-title">KUROOBI が使うファイル</div>
          {KINDS.map(([kind, title]) => {
            const info = byName.get(title);
            return (
              <div className="file-row" key={kind}>
                <div className="file-head">
                  {title}
                  {/* 使えているときは印だけ。足りないときだけ言葉で伝える */}
                  <span className={'file-state ' + (info?.ok ? 'ok' : 'ng')}>
                    <Icon name={info?.ok ? 'check' : 'alert'} size={13} />
                    {!info?.ok && 'ファイルがありません'}
                  </span>
                </div>
                <div className="file-path">{info?.p ?? '—'}</div>
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
                    onClick={() => onLearn(true)}>する</button>
            <button className={learnOn ? '' : 'active'}
                    onClick={() => onLearn(false)}>しない</button>
          </div>
        </div>
    </Modal>
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
  // コア数まで 1 刻み。飛び飛びにする理由は無い (奇数でも動くし、コアを
  // 1 つ空けたいこともある)。数が増えたので並びボタンからプルダウンへ
  const cores = th.auto * 2;
  return (
    <div className="settings-section">
      <div className="settings-title">ローカル探索のスレッド数</div>
      <p className="hint">
        ローカル対局・検討・学習の取り込みが使う並列数です。自動 = コア数の半分
        ({th.auto})。GGS 対局用は GGS の「KUROOBI の設定」にあります (別々に動く
        ので、両方が同時に動くと合計ぶんの CPU を使います)。
      </p>
      <div className="selwrap" style={{ alignSelf: 'flex-start', minWidth: 140 }}>
        <select value={th.set == null ? 'auto' : th.set}
                onChange={(e) => void set(e.target.value === 'auto' ? null : +e.target.value)}>
          <option value="auto">自動 ({th.auto})</option>
          {Array.from({ length: cores }, (_, i) => i + 1).map((n) => (
            <option key={n} value={n}>{n}</option>
          ))}
        </select>
      </div>
    </div>
  );
}
