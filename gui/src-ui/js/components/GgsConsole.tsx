// コンソール。通信ログの表示と生コマンドの送信。
import { useEffect, useRef, useState } from 'react';
import { ggsApi } from '../api';
import type { GgsCtx } from './GgsView';

export function GgsConsole({ ctx }: { ctx: GgsCtx }) {
  const { snap } = ctx;
  const [cmd, setCmd] = useState('');
  const box = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  // 末尾付近を見ているときだけ追従する (遡って読んでいる最中は動かさない)
  const count = snap.log.length;
  useEffect(() => {
    const b = box.current;
    if (b && stick.current) b.scrollTop = b.scrollHeight;
  }, [count]);

  const send = () => {
    const c = cmd.trim();
    if (!c) return;
    void ggsApi.raw(c);
    setCmd('');
  };

  return (
    <div className="ggs-cols">
      <div className="col-main">
        <div className="sec grow">
          <div className="sec-head"><h2>通信ログ</h2></div>
          <div className="scroll grow console-log" ref={box}
               onScroll={() => {
                 const b = box.current;
                 if (b) stick.current = b.scrollTop + b.clientHeight >= b.scrollHeight - 30;
               }}>
            {snap.log.map((l, i) => (
              <div key={i} className={'cl cl-' + l.dir}>
                {l.dir === 'out' && <span className="arrow">›</span>}
                {l.text}
              </div>
            ))}
          </div>
          <div className="row">
            <input type="text" value={cmd} onChange={(e) => setCmd(e.target.value)}
                   onKeyDown={(e) => { if (e.key === 'Enter') send(); }}
                   placeholder="コマンド (例: tell /os who 8)" />
            <button className="btn fix" onClick={send}>送信</button>
          </div>
        </div>
      </div>
    </div>
  );
}
