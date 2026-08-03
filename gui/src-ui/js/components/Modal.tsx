// ダイアログの外枠。ヘッダー・内容・アクションの 3 段で、
// 上下は固定して内容だけがスクロールする。
//
// 中身が長いダイアログ (設定・棋譜) で、閉じるボタンまで一緒に流れて
// いくのを避けるため。全ダイアログをこの形に揃えてある。
import type { ReactNode } from 'react';

export interface ModalProps {
  title: ReactNode;
  /** ヘッダーの見出しの下に添える短い説明 (任意)。 */
  subtitle?: ReactNode;
  children: ReactNode;
  /** アクション部に置くボタン。左端から並び、`spacer` で右へ寄せる。 */
  actions?: ReactNode;
  /** 幅の指定。既定は 440px。 */
  wide?: boolean;
  /** 背景をクリックしたときに閉じる (指定がなければ閉じない)。 */
  onClose?: () => void;
}

export function Modal({ title, subtitle, children, actions, wide, onClose }: ModalProps) {
  return (
    <div className="modal"
         onClick={onClose ? (e) => { if (e.target === e.currentTarget) onClose(); } : undefined}>
      <div className={'card box' + (wide ? ' wide' : '')}>
        <header className="modal-head">
          <h2>{title}</h2>
          {subtitle && <p className="modal-sub">{subtitle}</p>}
        </header>
        <div className="modal-body">{children}</div>
        {actions && <footer className="modal-foot">{actions}</footer>}
      </div>
    </div>
  );
}
