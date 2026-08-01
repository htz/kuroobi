// チャット。左に会話の一覧 (全体チャット + 話した相手ごと)、右に選んだ
// 会話。英語の発言は自動で和訳を添え、日本語の発言は英訳して送れる。
import { useEffect, useRef, useState } from 'react';
import { ggsApi, jsLog } from '../api';
import type { ChatMsg } from '../types';
import { clockOf, hasJapanese, translate } from '../ggs';
import type { GgsCtx } from './GgsView';

/** 訳文を添える対象か (英語の発言だけ)。 */
const wantsTranslation = (c: ChatMsg, login: string): boolean =>
  c.from !== login && !hasJapanese(c.text) && /[a-zA-Z]{2,}/.test(c.text);

const trKey = (c: ChatMsg): string => c.from + '|' + c.text;

export function GgsChat({ ctx }: { ctx: GgsCtx }) {
  const { snap } = ctx;
  const [thread, setThread] = useState('.chat');
  const [text, setText] = useState('');
  const [autoJa, setAutoJa] = useState(true);
  const [toEn, setToEn] = useState(false);
  // 訳文 ('' = 訳さない。原文と同じか取得に失敗)
  const [trs, setTrs] = useState<Record<string, string>>({});
  const log = useRef<HTMLDivElement>(null);

  // ---- 会話の一覧 (全体チャット + 話した相手ごと) ----
  const threads = new Map<string, { last: ChatMsg; n: number }>();
  threads.set('.chat', {
    last: { chan: '.chat', from: '', text: '', at: 0, thread: '.chat' }, n: 0,
  });
  for (const c of snap.chat) {
    const key = c.thread || (c.chan || c.from);
    const cur = threads.get(key);
    threads.set(key, { last: c, n: (cur?.n ?? 0) + 1 });
  }
  const cur = threads.has(thread) ? thread : '.chat';
  const sorted = [...threads.entries()].sort((a, b) => {
    if (a[0] === '.chat') return -1;
    if (b[0] === '.chat') return 1;
    return b[1].last.at - a[1].last.at;
  });
  const msgs = snap.chat.filter((c) => (c.thread || c.chan || c.from) === cur);

  // 新しい発言が来たら末尾へ (すでに末尾付近を見ているときだけ)
  const count = msgs.length;
  useEffect(() => {
    const box = log.current;
    if (!box) return;
    box.scrollTop = box.scrollHeight;
  }, [count, cur]);

  // 英語の発言に訳文を添える (取得は裏で、届いたら state 経由で再描画)
  const login = snap.login;
  const pendingTr = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (!autoJa) return;
    for (const c of msgs) {
      if (!wantsTranslation(c, login)) continue;
      const key = trKey(c);
      if (key in trs || pendingTr.current.has(key)) continue;
      pendingTr.current.add(key);
      translate(c.text, 'ja')
        .then((t) => setTrs((prev) => ({ ...prev, [key]: t && t !== c.text ? t : '' })))
        .catch(() => setTrs((prev) => ({ ...prev, [key]: '' })));
    }
  }, [msgs, autoJa, trs, login]);

  const translated = (c: ChatMsg): string | null => {
    if (!autoJa || !wantsTranslation(c, login)) return null;
    return trs[trKey(c)] || null;
  };

  const send = async () => {
    let t = text.trim();
    if (!t) return;
    setText('');
    if (toEn && hasJapanese(t)) {
      try { t = (await translate(t, 'en')) || t; } catch (e) { jsLog('翻訳失敗: ' + e); }
    }
    // 宛先は開いている会話 (全体チャット か 相手の名前)
    ggsApi.chat(cur, t).catch((e) => jsLog(String(e)));
  };

  const newTalk = () => {
    const names = snap.users.map((u) => u.name).filter((n) => n !== snap.login);
    if (!names.length) { alert('接続中のプレイヤーがいません。'); return; }
    const who = prompt(`誰に話しかけますか?\n\n接続中: ${names.join(', ')}`, names[0]);
    if (!who) return;
    const hit = names.find((n) => n.toLowerCase() === who.trim().toLowerCase());
    if (!hit) { alert(`${who} は接続していません。`); return; }
    setThread(hit);
  };

  // 日付の見出しと「同じ人が続けて話したら名前を省く」を先に計算する
  const rendered: { c: ChatMsg; day: string; dayHead: boolean; head: boolean }[] = [];
  {
    let lastFrom = '';
    let lastDay = '';
    for (const c of msgs) {
      const day = c.at ? new Date(c.at * 1000).toLocaleDateString('ja-JP',
        { month: 'long', day: 'numeric', weekday: 'short' }) : '';
      const dayHead = !!day && day !== lastDay;
      if (dayHead) { lastDay = day; lastFrom = ''; }
      const head = c.from !== lastFrom;
      lastFrom = c.from;
      rendered.push({ c, day, dayHead, head });
    }
  }

  return (
    <div className="chat-pane card">
      <aside className="chat-list">
        <div className="chat-list-head">
          <h2>会話</h2>
          <button className="btn small" onClick={newTalk}>新しく話す</button>
        </div>
        <div className="scroll grow">
          {sorted.map(([key, info]) => (
            <div key={key} className={'thread' + (key === cur ? ' active' : '')}
                 onClick={() => setThread(key)}>
              <div className="thread-top">
                <span className="thread-name">
                  {key.startsWith('.') ? '全体チャット' : key}
                </span>
                {info.last.at > 0 && (
                  <span className="thread-at">{clockOf(info.last.at)}</span>
                )}
              </div>
              <div className="thread-last">
                {info.n ? `${info.last.from}: ${info.last.text}` : 'まだ発言はありません'}
              </div>
            </div>
          ))}
        </div>
      </aside>

      <div className="chat-room">
        <div className="chat-room-head">
          <h2>{cur.startsWith('.') ? '全体チャット' : cur}</h2>
          <span className="muted">
            {cur.startsWith('.') ? 'ここにいる全員に届きます' : `${cur} だけに届きます`}
          </span>
          <span className="spacer" />
          <label className="check">
            <input type="checkbox" checked={autoJa}
                   onChange={(e) => setAutoJa(e.target.checked)} />
            和訳して表示
          </label>
          <label className="check">
            <input type="checkbox" checked={toEn}
                   onChange={(e) => setToEn(e.target.checked)} />
            英訳して送信
          </label>
        </div>
        <div className="chat-body" ref={log}>
          {!msgs.length && <div className="empty">まだ発言はありません。</div>}
          {rendered.map(({ c, day, dayHead, head }, i) => {
            const me = c.from === snap.login;
            const tr = translated(c);
            return (
              <div key={i} style={{ display: 'contents' }}>
                {dayHead && <div className="chat-day">{day}</div>}
                <div className={'msg' + (me ? ' me' : '') + (head ? '' : ' cont')}>
                  {head && (
                    <div className="msg-head">
                      <span className="msg-from">{me ? '自分' : c.from}</span>
                      {c.at > 0 && <span className="msg-at">{clockOf(c.at)}</span>}
                    </div>
                  )}
                  <div className="bubble">
                    <div className="msg-text">{c.text}</div>
                    {tr && <div className="msg-tr">{tr}</div>}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
        <form className="chat-compose" onSubmit={(e) => { e.preventDefault(); void send(); }}>
          <input type="text" value={text} onChange={(e) => setText(e.target.value)}
                 placeholder="メッセージを入力  (Enter で送信)" autoComplete="off" />
          <button className="btn primary" type="submit">送信</button>
        </form>
      </div>
    </div>
  );
}
