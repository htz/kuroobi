import React, { useEffect, useRef, useState } from 'react';
import { api, ggsApi, jsLog } from './api';
import type { ChatMsg, GgsSnapshot, MatchView } from './types';
import {
  CLOCK_CHOICES, GTYPE_CHOICES, clockOf, countDiscs, ggsMoveToIndex, gtypeLabel,
  fingerValue, hasJapanese, normKey, parseCond, translate, useClocks,
  type ClockSide, type ClockView,
} from './ggs';
import { EmptyState, Section } from './components/layout';
import { Button, Segmented, Select, TextField, Toggle } from './components/primitives';
import { Strength } from './components/strength';
import { Confirm, PickOne } from './Dialogs';
import { IconButton } from './components/Icons';
import {
  Bubble, ConsoleLog, DayMark, FormulaEditor, FormulaView, MatchRow, PlayerRow, RateRow, Tag,
  type Cond, type Match, type NavId,
} from './components/ggs';
import { Board, type Cell } from './components/board';
import { flipped, type Prefs } from './prefs';
import { logLinesOf } from './adapt';

/* GGS の画面。左メニューの行き先ごとに中身を出し分ける。
 *
 * 未接続のときは 7 行を出さずログイン 1 行だけになるので、ここへ来るのは
 * ログイン画面だけ。残りは順に置き換えていく。
 */

export function GgsScreen({ nav, snap, onNav, prefs }: {
  nav: NavId; snap: GgsSnapshot | null; onNav: (id: NavId) => void; prefs: Prefs;
}) {
  if (nav === 'ggs-login') return <GgsLogin />;
  if (!snap) return <EmptyState title="GGS に接続していません" />;

  switch (nav) {
    case 'ggs-play': return <GgsPlay snap={snap} onNav={onNav} prefs={prefs} />;
    case 'ggs-lobby': return <GgsLobby snap={snap} onNav={onNav} />;
    case 'ggs-players': return <GgsUsers snap={snap} onNav={onNav} />;
    case 'ggs-chat': return <GgsChat snap={snap} />;
    case 'ggs-standby': return <GgsStandby snap={snap} onNav={onNav} />;
    case 'ggs-console': return <GgsConsole snap={snap} />;
    case 'ggs-settings': return <GgsSettings snap={snap} />;
    default: return null;
  }
}

/* 浮く箱の中の入力ラベル。節 (1px 罫つきの見出し) はコンテンツ領域のもので、
 * 340px の箱の中に置くと罫が箱を横切って見出しに見えてしまう */
function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)', alignItems: 'flex-start' }}>
      <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{label}</span>
      {children}
    </label>
  );
}

/* ---------------- ログイン ----------------
 *
 * 保存済みの認証情報は起動時の自動ログインが使うので、この画面が出るのは
 * 未保存のとき・自動ログインに失敗したとき・ログアウトした後だけ。
 *
 * 丸四角の箱を内容に使わないのが決まりだが、ここは例外 — 画面に 1 つしか
 * ない入口で、他に並ぶものが無いので、浮かせたほうが目的地だと分かる。
 */
function GgsLogin() {
  const [user, setUser] = useState('');
  const [pw, setPw] = useState('');
  const [status, setStatus] = useState('');

  const connect = async () => {
    setStatus('接続しています…');
    try {
      await ggsApi.connect(user, pw);
      setStatus('');
    } catch (e) {
      setStatus(String(e));
    }
  };

  return (
    <div style={{ flex: 1, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)' }}>
      <div style={{
        width: 340, borderRadius: 'var(--r-4)', background: 'var(--card)',
        border: '1px solid var(--border)', boxShadow: 'var(--sh-2)',
        padding: 'var(--sp-5)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-4)',
      }}>
        <div style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>GGS へログイン</div>
        <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.7 }}>
          skatgame.net:5000 — ログインに成功するとキーチェーンに保存され、次回から自動ログインします
        </div>
        <Field label="ログイン名"><TextField value={user} onChange={setUser} /></Field>
        <Field label="パスワード"><TextField value={pw} password onChange={setPw} /></Field>
        <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
          <span style={{ flex: 1, fontSize: 'var(--fs-6)', color: 'var(--bad)' }}>{status}</span>
          <Button variant="primary" onClick={() => void connect()}>ログイン</Button>
        </div>
      </div>
    </div>
  );
}

/* ---------------- コンソール ----------------
 *
 * 通信ログと生コマンド。GGS は画面に出していない機能が多いので、
 * 逃げ道として直に打てる場所を必ず残す。
 */
function GgsConsole({ snap }: { snap: GgsSnapshot }) {
  const [cmd, setCmd] = useState('');
  const send = () => {
    const c = cmd.trim();
    if (!c) return;
    void ggsApi.raw(c);
    setCmd('');
  };
  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', padding: '0 var(--sp-4)' }}>
        <Section title="通信ログ" />
        <ConsoleLog lines={logLinesOf(snap.log)} />
      </div>
      <div style={{
        flex: 'none', display: 'flex', gap: 'var(--sp-2)', alignItems: 'center',
        padding: 'var(--sp-3) var(--sp-4)', borderTop: '1px solid var(--border-weak)',
      }}>
        <TextField mono value={cmd} onChange={setCmd}
                   placeholder="コマンド (例: tell /os who 8)" />
        <Button onClick={send}>送信</Button>
      </div>
    </div>
  );
}

/* ---------------- チャット ----------------
 *
 * 左に会話の一覧 (全体チャット + 話した相手ごと)、右に選んだ会話。
 * 英語の発言は自動で和訳を添え、日本語の発言は英訳して送れる。
 */

/** 訳文を添える対象か (自分以外の英語の発言だけ)。 */
const wantsTranslation = (c: ChatMsg, login: string): boolean =>
  c.from !== login && !hasJapanese(c.text) && /[a-zA-Z]{2,}/.test(c.text);

const trKey = (c: ChatMsg): string => c.from + '|' + c.text;

function GgsChat({ snap }: { snap: GgsSnapshot }) {
  const [thread, setThread] = useState('.chat');
  const [text, setText] = useState('');
  const [autoJa, setAutoJa] = useState(true);
  const [pick, setPick] = useState(false);
  const [toEn, setToEn] = useState(false);
  // 訳文 ('' = 訳さない。原文と同じか取得に失敗)
  const [trs, setTrs] = useState<Record<string, string>>({});
  const pending = useRef<Set<string>>(new Set());
  const box = useRef<HTMLDivElement>(null);

  // 会話の一覧。全体チャットは必ず先頭、あとは新しい順
  const threads = new Map<string, { last: ChatMsg; n: number }>();
  threads.set('.chat', { last: { chan: '.chat', from: '', text: '', at: 0, thread: '.chat' }, n: 0 });
  for (const c of snap.chat) {
    const key = c.thread || c.chan || c.from;
    threads.set(key, { last: c, n: (threads.get(key)?.n ?? 0) + 1 });
  }
  const cur = threads.has(thread) ? thread : '.chat';
  const sorted = [...threads.entries()].sort((a, b) =>
    a[0] === '.chat' ? -1 : b[0] === '.chat' ? 1 : b[1].last.at - a[1].last.at);
  const msgs = snap.chat.filter((c) => (c.thread || c.chan || c.from) === cur);

  const login = snap.login;
  const count = msgs.length;
  useEffect(() => {
    const b = box.current;
    if (b) b.scrollTop = b.scrollHeight;
  }, [count, cur]);

  // 英語の発言に訳文を添える (取得は裏で、届いたら再描画)
  useEffect(() => {
    if (!autoJa) return;
    for (const c of msgs) {
      if (!wantsTranslation(c, login)) continue;
      const key = trKey(c);
      if (key in trs || pending.current.has(key)) continue;
      pending.current.add(key);
      translate(c.text, 'ja')
        .then((t) => setTrs((prev) => ({ ...prev, [key]: t && t !== c.text ? t : '' })))
        .catch(() => setTrs((prev) => ({ ...prev, [key]: '' })));
    }
  }, [msgs, autoJa, trs, login]);

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

  // 日付の見出しと「同じ人が続けて話したら名前を省く」を先に決める
  const rows: { c: ChatMsg; day: string; dayHead: boolean; head: boolean }[] = [];
  let lastFrom = '', lastDay = '';
  for (const c of msgs) {
    const day = c.at ? new Date(c.at * 1000).toLocaleDateString('ja-JP',
      { month: 'long', day: 'numeric', weekday: 'short' }) : '';
    const dayHead = !!day && day !== lastDay;
    if (dayHead) { lastDay = day; lastFrom = ''; }
    const head = c.from !== lastFrom;
    lastFrom = c.from;
    rows.push({ c, day, dayHead, head });
  }

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      {/* 会話の一覧。幅は手合い一覧と同じ --w-list に揃える */}
      <aside style={{
        width: 'var(--w-list)', flex: 'none', borderRight: '1px solid var(--border)',
        minHeight: 0, display: 'flex', flexDirection: 'column',
      }}>
        <div style={{
          flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
          padding: 'var(--sp-2) var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
        }}>
          <span style={{ fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)' }}>会話</span>
          <span style={{ marginLeft: 'auto' }} />
          <Button size="chip" onClick={() => setPick(true)}>新しい相手</Button>
        </div>
        <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>
        {sorted.map(([key, info]) => (
          <button key={key} type="button" onClick={() => setThread(key)}
            aria-current={key === cur || undefined}
            className={'k-row' + (key === cur ? ' k-on' : '')}
            style={{
              width: '100%', border: 0, textAlign: 'left', display: 'flex', flexDirection: 'column', gap: 3,
              padding: '9px var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
              background: key === cur ? 'color-mix(in srgb, var(--accent) 14%, transparent)' : 'transparent',
              boxShadow: key === cur ? 'inset 2px 0 0 var(--accent)' : 'none',
            }}>
            <span style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 'var(--fs-5)' }}>
              {key === '.chat' ? '全体チャット' : key}
              {info.last.at > 0 && (
                <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>
                  {clockOf(info.last.at)}
                </span>
              )}
            </span>
            <span style={{
              fontSize: 'var(--fs-6)', color: 'var(--sub)',
              overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
            }}>{info.last.text || '—'}</span>
          </button>
        ))}
        </div>
      </aside>

      <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
        <div className="k-scroll" ref={box} style={{
          flex: 1, minHeight: 0, padding: 'var(--sp-4)',
          display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)',
        }}>
          {rows.map(({ c, day, dayHead, head }, i) => (
            <React.Fragment key={i}>
              {dayHead && <DayMark>{day}</DayMark>}
              <Bubble showName={head} m={{
                from: c.from, mine: c.from === login, at: clockOf(c.at), body: c.text,
                ja: autoJa && wantsTranslation(c, login) ? (trs[trKey(c)] || undefined) : undefined,
              }} />
            </React.Fragment>
          ))}
        </div>
        <div style={{
          flex: 'none', display: 'flex', gap: 'var(--sp-2)', alignItems: 'center',
          padding: 'var(--sp-3) var(--sp-4)', borderTop: '1px solid var(--border-weak)',
        }}>
          <Toggle checked={autoJa} onChange={setAutoJa} label="和訳" />
          <Toggle checked={toEn} onChange={setToEn} label="英訳して送る" />
          <TextField value={text} onChange={setText}
                     placeholder={cur === '.chat' ? '全体チャットへ' : `${cur} へ`} />
          <Button variant="primary" onClick={() => void send()}>送信</Button>
        </div>
      </div>
      {pick && (
        <PickOne title="誰に話しかけますか?"
                 body={<span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                   接続中の人だけを出しています。
                 </span>}
                 options={snap.users.filter((u) => u.name !== login).map((u) => [u.name, u.name] as [string, string])}
                 onCancel={() => setPick(false)}
                 onOk={(who) => { setThread(who); setPick(false); }} />
      )}
    </div>
  );
}

/* ---------------- ロビー ----------------
 *
 * 進行中の対局 (観戦できる)・対局の申し込み・申し込みフォーム・中断対局。
 * 左が一覧、右が「これから始める」もの。幅は --w-lobby。
 */
function GgsLobby({ snap, onNav }: { snap: GgsSnapshot; onNav: (id: NavId) => void }) {
  const [opp, setOpp] = useState('');
  const [gtype, setGtype] = useState('s8r16');
  const [time, setTime] = useState('00:15:00');

  const games = snap.ongoing.filter((o) => !o.mine);
  const names = snap.users.filter((u) => u.name !== snap.login).map((u) => u.name);

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <div className="k-scroll" style={{ flex: 1, minWidth: 0, padding: 'var(--sp-4) var(--sp-2) 0' }}>
        <Section title="対局中" aside={games.length ? `${games.length} 局` : undefined}>
          {!games.length && <Empty>進行中の対局はありません。</Empty>}
          {games.map((o) => (
            <Row key={o.id}
                 title={`${o.names[0] || '?'} 対 ${o.names[1] || '?'}`}
                 sub={gtypeLabel(o.gtype)}
                 actions={
                   <Button size="chip" variant={o.watching ? 'danger' : 'primary'}
                           onClick={() => {
                             const on = !o.watching;
                             void ggsApi.watch(o.id, on);
                             // 観戦を始めたら盤が見たいはずなので対局画面へ移る
                             if (on) onNav('ggs-play');
                           }}>
                     {o.watching ? '観戦をやめる' : '観戦'}
                   </Button>} />
          ))}
        </Section>

        <Section title="対局の申し込み">
          {!snap.offers.length && <Empty>対局の申し込みはありません。</Empty>}
          {snap.offers.map((o) => {
            const who = o.names.filter((n) => n !== snap.login);
            return (
              <Row key={o.id}
                   title={who.join(' と ') || '?'}
                   tag={o.incoming ? '自分宛' : undefined}
                   sub={`${gtypeLabel(o.gtype)} · ${o.time || '?'}${o.rated ? ' · レート戦' : ''}`}
                   actions={o.incoming ? <>
                     <Button size="chip" variant="primary" onClick={() => void ggsApi.accept(o.id)}>受ける</Button>
                     <Button size="chip" variant="danger" onClick={() => void ggsApi.decline(o.id)}>断る</Button>
                   </> : undefined} />
            );
          })}
        </Section>
      </div>

      <aside className="k-scroll" style={{
        // 右の列はドックと同じ幅に揃える。画面ごとに違う幅を作ると、
        // 行き来したときに本体の幅が動いて落ち着かない
        width: 'var(--w-dock)', flex: 'none', borderLeft: '1px solid var(--border)',
        padding: 'var(--sp-4) var(--sp-2) 0', minHeight: 0,
      }}>
        <Section title="対局を申し込む">
          <Field label="相手">
            <Select value={names.includes(opp) ? opp : ''} onChange={setOpp}
                    options={[['', '指定しない (誰でも)'], ...names.map((n) => [n, n] as [string, string])]} />
          </Field>
          <Field label="形式"><Select value={gtype} onChange={setGtype} options={GTYPE_CHOICES} /></Field>
          <Field label="持ち時間"><Select value={time} onChange={setTime} options={CLOCK_CHOICES} /></Field>
          {/* 相手を指定しない申し込みは「誰でも受けられる」募集になる。
              GGS の /os ask はそういう使い方ができるので、止めない */}
          <Button variant="primary"
                  onClick={() => void ggsApi.ask(gtype, time, opp)}>
            {opp ? '申し込む' : '募集する'}
          </Button>
          <p style={{ margin: 0, fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8 }}>
            同期対局は同じ開局を先後入れ替えて 2 局同時に行い、結果は合計で判定します。
            レートは「ランダム開局」に反映されます。
          </p>
        </Section>

        <Section title="中断対局">
          {!snap.stored.length && <Empty>中断対局はありません。</Empty>}
          {snap.stored.map((x) => (
            <Row key={x.id} title={x.opp || '?'} sub={gtypeLabel(x.gtype)}
                 actions={<Button size="chip" variant="primary"
                                  onClick={() => void ggsApi.resumeStored(x.id)}>再開</Button>} />
          ))}
        </Section>
      </aside>
    </div>
  );
}

/** 一覧の 1 行。名前と補足が左、操作が右。行そのものは押さない */
function Row({ title, sub, tag, actions }: {
  title: string; sub?: string; tag?: string; actions?: React.ReactNode;
}) {
  return (
    <div style={{
      display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
      padding: '7px 0', borderBottom: '1px solid var(--border-weak)',
    }}>
      <span style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 2 }}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 'var(--fs-5)' }}>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{title}</span>
          {tag && <Tag tone="accent">{tag}</Tag>}
        </span>
        {sub && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{sub}</span>}
      </span>
      {actions}
    </div>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <div style={{ padding: '10px 0', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{children}</div>;
}

/* ---------------- 待機モード ----------------
 *
 * 対局終了 → 間隔待ち → 自動申し込みの繰り返し。
 * サーバー側の申し込み条件は今の値を見せるだけで、編集は「GGS の設定」。
 * 同じ設定を 2 か所で編集できると、どちらが本物か分からなくなる。
 */
function GgsStandby({ snap, onNav }: { snap: GgsSnapshot; onNav: (id: NavId) => void }) {
  const sb = snap.standby;
  const st = snap.standby_stats;
  const [opp, setOpp] = useState(sb.opponent);
  const [gtype, setGtype] = useState(sb.gtype || 's8r16');
  const [time, setTime] = useState(sb.time || '00:15:00');
  const [maxGames, setMaxGames] = useState(sb.max_games);
  const [interval, setInterval] = useState(sb.interval_secs || 20);
  const [autoAccept, setAutoAccept] = useState(sb.auto_accept);

  const names = snap.users.filter((u) => u.name !== snap.login).map((u) => u.name);
  const state = sb.enabled ? (snap.matches.length ? '対局中' : '申し込み待ち') : '停止中';

  const toggle = () => void ggsApi.setStandby({
    enabled: !sb.enabled, auto_accept: autoAccept, opponent: opp.trim(),
    gtype, time, max_games: maxGames, interval_secs: interval,
  });

  // 自分の設定を取り直す (条件式はサーバーが持っている)
  useEffect(() => {
    if (snap.login) ggsApi.finger(snap.login).catch(() => {});
  }, [snap.login]);

  const form = (key: 'accept' | 'decline'): string =>
    (snap.fingers[snap.login]?.fields
      .find(([k]) => k.replace(/\s+/g, '').replace(/\(.*\)/, '') === key)?.[1] ?? '')
      .replace(/^\s*:\s*/, '').trim();

  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4) var(--sp-4) 0' }}>
      <Section title="連戦の待機"
               aside={<span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
                 <Tag tone={sb.enabled ? 'ok' : 'sub'}>{state}</Tag>
                 <Stat v={st.games} label="局" />
                 <Stat v={st.wins} label="勝" color="var(--ok)" />
                 <Stat v={st.losses} label="敗" color="var(--bad)" />
                 <Stat v={st.draws} label="分" />
                 <Stat v={`${st.diff_sum > 0 ? '+' : ''}${st.diff_sum}`} label="石差" />
               </span>}>
        <div style={{ display: 'flex', gap: 'var(--sp-4)', flexWrap: 'wrap' }}>
          <Field label="相手">
            <Select value={names.includes(opp) ? opp : ''} onChange={setOpp} width={180}
                    options={[['', '指定しない (誰でも)'], ...names.map((n) => [n, n] as [string, string])]} />
          </Field>
          <Field label="形式"><Select value={gtype} onChange={setGtype} width={200} options={GTYPE_CHOICES} /></Field>
          <Field label="持ち時間"><Select value={time} onChange={setTime} width={110} options={CLOCK_CHOICES} /></Field>
          <Field label="最大対局数 (0 = 無制限)">
            <TextField numeric align="right" width={80} value={String(maxGames)}
                       onChange={(x) => setMaxGames(+x || 0)} />
          </Field>
          <Field label="対局の間隔 (秒)">
            <TextField numeric align="right" width={80} value={String(interval)}
                       onChange={(x) => setInterval(+x || 0)} />
          </Field>
        </div>
        {/* Toggle は摘みを右端へ寄せる作りなので、幅を決めずに置くと画面の端まで離れる */}
        <span style={{ width: 300, display: 'block' }}>
          <Toggle checked={autoAccept} onChange={setAutoAccept} label="届いた申し込みを自動で受ける" />
        </span>
        <div>
          <Button variant={sb.enabled ? 'danger' : 'primary'} onClick={toggle}>
            {sb.enabled ? '待機モードを停止' : '待機モードを開始'}
          </Button>
        </div>
        <p style={{ margin: 0, fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8 }}>
          対局終了 → 間隔待ち → 自動申し込みを繰り返します。切断時は自動で再接続し、
          中断対局も自動再開します。相手を指定しないときは自分からは申し込まず、
          届いた申し込みを受けるだけになります。
        </p>
      </Section>

      <Section title="申し込みの扱い (サーバー側)"
               aside={<Button size="chip" onClick={() => onNav('ggs-settings')}>条件を変える</Button>}>
        <p style={{ margin: 0, fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8 }}>
          条件は GGS のサーバーが持っています。ここでは今の値を見るだけ —
          同じ設定を 2 か所で編集できると、どちらが本物か分からなくなります。
        </p>
        <FormulaRow label="自動で受ける条件" src={form('accept')} />
        <FormulaRow label="自動で断る条件" src={form('decline')} />
      </Section>
    </div>
  );
}

function Stat({ v, label, color }: { v: number | string; label: string; color?: string }) {
  return (
    <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', letterSpacing: 0 }}>
      <b style={{ color: color ?? 'var(--text)', fontWeight: 600 }}>{v}</b>{label}
    </span>
  );
}

function FormulaRow({ label, src }: { label: string; src: string }) {
  const cond = src ? parseCond(src) : null;
  return (
    <div style={{ display: 'flex', gap: 'var(--sp-3)', alignItems: 'flex-start' }}>
      <span style={{ width: 'var(--w-label)', flex: 'none', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{label}</span>
      {cond ? <FormulaView node={cond} top />
            : <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>指定なし</span>}
    </div>
  );
}

/* ---------------- 対局・観戦 ----------------
 *
 * 同期対局は 2 局で 1 組。組を 1 行として扱い、右に盤を並べる。
 * 待った (undo) と中止 (abort) は出さない — どちらも相手の承諾が要る要求で、
 * GGS の相手はたいていプログラムなので通らない。投了だけは自分で決められる。
 */
function GgsPlay({ snap, onNav, prefs }: { snap: GgsSnapshot; onNav: (id: NavId) => void; prefs: Prefs }) {
  const [sel, setSel] = useState('');
  const clock = useClocks(snap.matches);

  // 手合いにまとめる。自分の対局を先に、次に観戦
  const groups = new Map<string, MatchView[]>();
  for (const m of snap.matches) groups.set(m.base, [...(groups.get(m.base) ?? []), m]);
  const keys = [...groups.keys()].sort((a, b) => {
    const mine = (k: string) => (groups.get(k)!.some((m) => m.my_color) ? 0 : 1);
    return mine(a) - mine(b) || a.localeCompare(b);
  });
  const cur = groups.has(sel) ? sel : keys[0] ?? '';
  const pair = groups.get(cur);

  if (!groups.size) {
    return (
      <EmptyState title="対局はまだありません"
                  body="ロビーで申し込むか、進行中の対局を観戦できます。"
                  actions={<Button variant="primary" onClick={() => onNav('ggs-lobby')}>ロビーへ</Button>} />
    );
  }

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <aside className="k-scroll" style={{
        width: 'var(--w-list)', flex: 'none', borderRight: '1px solid var(--border)', minHeight: 0,
      }}>
        {keys.map((key) => (
          <MatchRow key={key} m={matchRowOf(groups.get(key)!, key)}
                    active={key === cur} onSelect={() => setSel(key)} />
        ))}
      </aside>

      <div className="k-scroll" style={{ flex: 1, minWidth: 0, minHeight: 0, padding: 'var(--sp-3)' }}>
        {/* 同期対局は 2 面。横に並べて、狭ければ折り返す */}
        <div style={{ display: 'flex', gap: 'var(--sp-3)', flexWrap: 'wrap', justifyContent: 'center' }}>
          {pair?.map((m) => <MatchBoard key={m.id} snap={snap} m={m} clock={clock} prefs={prefs} />)}
        </div>
      </div>
    </div>
  );
}

/** 手合い 1 組を一覧の行にする。終局しても一覧からは消さない */
function matchRowOf(g: MatchView[], key: string): Match {
  const m = g[0];
  const mine = g.some((x) => x.my_color);
  return {
    id: key, mine, live: !g.every((x) => x.over),
    opponent: m.opp_name,
    black: m.players[0]?.name ?? '?', white: m.players[1]?.name ?? '?',
    kind: gtypeLabel(m.gtype), boards: g.length,
    ply: Math.max(...g.map((x) => x.moves.length)),
    result: g.map((x) => x.result).find(Boolean) || undefined,
  };
}

function MatchBoard({ snap, m, clock, prefs }: {
  snap: GgsSnapshot; m: MatchView; clock: (id: string, side: ClockSide) => ClockView; prefs: Prefs;
}) {
  const [resign, setResign] = useState(false);
  const observer = !m.my_color;
  const { black, white } = countDiscs(m.cells);
  // 最後に打たれた石に印を付ける (パスは石を置かないので飛ばす)
  const last = [...m.moves].reverse().map(ggsMoveToIndex).find((x) => x !== null) ?? null;
  // 自分の側もレートを出す。対局中に見たいのは「この 2 人の力量差」
  const myRate = snap.my_ranks.find((r) => r.gtype === (m.gtype.includes('r') ? '8r' : '8'))?.rating;
  const myEval = m.last_eval != null
    ? (m.last_from_book ? '定石 ' : '') + (m.last_eval > 0 ? '+' : '')
      + (m.last_eval_exact ? m.last_eval.toFixed(0) : m.last_eval.toFixed(1))
      + (m.last_eval_exact ? ' 読切' : '')
    : undefined;

  const top = observer && m.players.length >= 2
    ? { name: m.players[0].name, rate: m.players[0].rating, color: m.players[0].color, side: 'p0' as const }
    : { name: m.opp_name, rate: m.opp_rating, color: m.my_color === 'black' ? 'white' as const : 'black' as const, side: 'opp' as const };
  const bottom = observer && m.players.length >= 2
    ? { name: m.players[1].name, rate: m.players[1].rating, color: m.players[1].color, side: 'p1' as const }
    : { name: snap.login, rate: myRate != null ? myRate.toFixed(1) : '', color: m.my_color as 'black' | 'white', side: 'my' as const };

  return (
    // 同期対局は 2 面が横に並ぶ。固定幅にすると 2 面で必ず折り返すので、
    // 器を分け合って縮む形にする (1 面のときは 460px で頭打ち)
    <div style={{ flex: '1 1 300px', minWidth: 260, maxWidth: 460, display: 'flex', flexDirection: 'column' }}>
      <PlayerRow color={top.color === 'black' ? 'b' : 'w'} name={top.name || '?'}
                 rate={top.rate ? +top.rate : undefined}
                 clock={clock(m.id, top.side).text} active={clock(m.id, top.side).cls === 'turn'} />
      {/* 「自分が下」は自分の色を下にする。観戦は my_color が空なので黒が下 */}
      <Board cells={m.cells as Cell[]} last={last} disabled
             coords={prefs.coords} grain={prefs.grain}
             flip={flipped(prefs.facing, m.my_color)} />
      <PlayerRow color={bottom.color === 'black' ? 'b' : 'w'} name={bottom.name || '?'}
                 rate={bottom.rate ? +bottom.rate : undefined}
                 meta={myEval}
                 clock={clock(m.id, bottom.side).text} active={clock(m.id, bottom.side).cls === 'turn'} />
      <div style={{
        display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-field)',
        fontSize: 'var(--fs-6)', color: 'var(--sub)',
      }}>
        <span style={{ color: 'var(--text)' }}>{black} – {white}</span>
        <span>{m.moves.length} 手</span>
        {observer && m.watch_eval != null && (
          <span>解析 {m.watch_eval > 0 ? '+' : ''}{m.watch_eval.toFixed(1)}
            {m.watch_best ? ` (${m.watch_best})` : ''}</span>
        )}
        {snap.thinking === m.id && <span style={{ color: 'var(--accent)' }}>思考中</span>}
        <span style={{ marginLeft: 'auto' }} />
        {!observer && (
          <Button size="chip" variant="danger"
                  title="負けを認めて終わる (相手の承諾は要らない。レートが動く)"
                  onClick={() => setResign(true)}>投了</Button>
        )}
      </div>
      {resign && (
        <Confirm title="投了しますか?" ok="投了する" danger
                 body={<>負けになり、レートが動きます。相手の承諾は要りません。</>}
                 onCancel={() => setResign(false)}
                 onOk={() => { setResign(false); void ggsApi.matchCmd(m.id, 'resign'); }} />
      )}
    </div>
  );
}

/* ---------------- GGS の設定 ----------------
 *
 * GGS で対局・観戦するときの強さとふるまい。ローカル対局とは別に持つ。
 * 申し込みの条件は**サーバー側に残る**ので、アプリを閉じていても効く。
 */
function GgsSettings({ snap }: { snap: GgsSnapshot }) {
  const e = snap.engine;
  const [saved, setSaved] = useState(false);
  const [levels, setLevels] = useState({ depth: e.depth, solve: e.solve, band: e.band });
  const [threads, setThreads] = useState(e.threads);
  const [auto, setAuto] = useState(snap.auto_play);
  const [watch, setWatch] = useState(snap.watch_analysis);
  const [book, setBook] = useState(e.use_book);
  const [pace, setPace] = useState(e.pace || 'even');
  const [maxMove, setMaxMove] = useState(e.max_move_secs);
  const [reserve, setReserve] = useState(e.reserve_secs);
  const [cores, setCores] = useState(0);
  useEffect(() => { api.activity().then((a) => setCores(a.cores)).catch(() => {}); }, []);

  // 申し込みの条件はサーバーが持っている。開いたときに取り直す
  const login = snap.login;
  useEffect(() => { if (login) ggsApi.finger(login).catch(() => {}); }, [login]);
  const myForm = (key: 'accept' | 'decline'): string =>
    snap.fingers[login]?.fields
      .find(([k]) => k.replace(/\s+/g, '').replace(/\(.*\)/, '') === key)?.[1] ?? '';
  const saveForm = async (kind: 'aform' | 'dform', expr: string) => {
    await ggsApi.setFormula(kind, expr).catch(() => {});
    // 送っただけで信じない。サーバーの値を取り直して画面に反映する
    if (login) ggsApi.finger(login).catch(() => {});
  };

  const apply = async () => {
    await ggsApi.setEngine(levels.depth, levels.solve, levels.band, threads).catch(() => {});
    await ggsApi.setPacing(pace, maxMove, reserve).catch(() => {});
    await ggsApi.setAutoPlay(auto).catch(() => {});
    await ggsApi.setWatchAnalysis(watch).catch(() => {});
    await ggsApi.setUseBook(book).catch(() => {});
    setSaved(true);
    window.setTimeout(() => setSaved(false), 2000);
  };

  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4) var(--sp-4) 0' }}>
      <div style={{ maxWidth: 720, display: 'flex', flexDirection: 'column' }}>
        <Section title="強さ">
          <Note>読む深さの上限です。どこまで読めるかは下の持ち時間の使い方が決めます。</Note>
          <span style={{ maxWidth: 340, display: 'block' }}>
            <Strength value={levels} onChange={setLevels} />
          </span>
          {/* コア数まで 1 刻み。飛び飛びにする理由は無い (奇数でも動くし、
              コアを 1 つ空けたいことはある)。
              0 は「自動」の印 — 解決した数ではなく 0 のまま持つので、
              コア数の違う機械へ設定を移しても自動のまま */}
          <Field label="スレッド">
            <Select width={120} size="ctrl" value={String(threads)}
                    onChange={(s) => setThreads(+s)}
                    options={[['0', `自動 (${Math.max(1, Math.floor(cores / 2)) || '—'})`],
                              ...Array.from({ length: cores || Math.max(threads, 1) }, (_, i) =>
                                [String(i + 1), String(i + 1)] as [string, string])]} />
          </Field>
        </Section>

        <Section title="持ち時間の使い方">
          <Note>
            1 手にかける時間を、残り時間と残り手数から決めます。時間内で読める深さまで読み、
            時間が来たらそこまでの答えで指します。
          </Note>
          <div><Segmented value={pace} onChange={setPace} options={[
            { value: 'slow', label: 'じっくり' }, { value: 'even', label: '均等' },
            { value: 'fast', label: '速指し' }, { value: 'depth', label: '深さ固定' },
          ]} /></div>
          <Note>
            {pace === 'depth' ? '時間を見ずに上の深さまで読みます。持ち時間の管理は自分で行うことになります。'
              : pace === 'slow' ? '序盤に厚く配ります。研究向き。'
              : pace === 'fast' ? '序盤を短く切り上げ、終盤に残します。'
              : '残り手数で等分します。'}
          </Note>
          {pace !== 'depth' && (
            <div style={{ display: 'flex', gap: 'var(--sp-4)' }}>
              <Field label="1 手の上限 (秒、0 = なし)">
                <TextField numeric align="right" width={80} value={String(maxMove)}
                           onChange={(x) => setMaxMove(Math.max(0, +x || 0))} />
              </Field>
              <Field label="読み切り用に残す (秒)">
                <TextField numeric align="right" width={80} value={String(reserve)}
                           onChange={(x) => setReserve(Math.max(0, +x || 0))} />
              </Field>
            </div>
          )}
        </Section>

        <Section title="定石">
          {e.book_loaded
            ? <div><Segmented value={book ? 'on' : 'off'} onChange={(v) => setBook(v === 'on')}
                              options={[{ value: 'on', label: '使う' }, { value: 'off', label: '使わない' }]} /></div>
            : <Note>ファイルがありません。左メニュー下の「設定」で指定してください。</Note>}
        </Section>

        <Section title="申し込みの扱い">
          <Note>
            相手から対局を申し込まれたときに、自動で受ける / 断る条件です。
            <b style={{ color: 'var(--text)' }}>サーバー側に残る</b>ので、アプリを閉じていても効きます。
            受ける条件と断る条件の両方に当てはまるときは、断るほうが勝ちます。
          </Note>
          <FormulaField label="自動で受ける条件" src={myForm('accept')}
                        onSave={(s) => void saveForm('aform', s)} />
          <FormulaField label="自動で断る条件" src={myForm('decline')}
                        onSave={(s) => void saveForm('dform', s)} />
        </Section>

        <Section title="ふるまい">
          <span style={{ width: 300, display: 'block' }}>
            <Toggle checked={auto} onChange={setAuto} label="自分の手番で自動的に指す" />
          </span>
          <span style={{ width: 300, display: 'block' }}>
            <Toggle checked={watch} onChange={setWatch} label="観戦中の対局も解析する" />
          </span>
        </Section>

        <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', padding: '0 var(--sp-3) var(--sp-5)' }}>
          {saved && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--ok)' }}>反映しました</span>}
          <span style={{ marginLeft: 'auto' }} />
          <Button variant="primary" onClick={() => void apply()}>適用</Button>
        </div>

        <Section title="接続">
          <Note>
            ログアウトすると保存済みの認証情報も消えます。次の起動では自動ログインしません。
          </Note>
          <div><Button variant="danger" onClick={() => void ggsApi.disconnect()}>ログアウト</Button></div>
        </Section>
      </div>
    </div>
  );
}

function Note({ children }: { children: React.ReactNode }) {
  return <p style={{ margin: 0, fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8 }}>{children}</p>;
}

/** 条件式 1 つ。木のまま組ませ、保存するときだけ文字列に戻す */
function FormulaField({ label, src, onSave }: { label: string; src: string; onSave: (s: string) => void }) {
  const [cond, setCond] = useState<Cond | null>(() => (src ? parseCond(src) : null));
  return (
    <Field label={label}>
      <FormulaEditor value={cond} onChange={setCond} onClear={() => setCond(null)}
                     onSave={onSave} />
    </Field>
  );
}

/* ---------------- プレイヤー ----------------
 *
 * 接続中の一覧と、選んだ人の詳細。詳細の頭には名前だけでなくレートと
 * 対局の状況も載せる — 一覧で見えていたものが詳細で消えると、
 * 申し込む前に一覧へ戻る羽目になる。
 */
function GgsUsers({ snap, onNav }: { snap: GgsSnapshot; onNav: (id: NavId) => void }) {
  const [sel, setSel] = useState<string | null>(null);
  const [mode, setMode] = useState<'who' | 'top'>('who');
  const [tab, setTab] = useState('プロフィール');

  if (sel) {
    return <UserDetail snap={snap} name={sel} tab={tab} onTab={setTab}
                       onBack={() => setSel(null)} onNav={onNav} />;
  }

  const rows = mode === 'who' ? snap.users : snap.ranking;
  const mine = snap.my_ranks;

  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4) var(--sp-4) 0' }}>
      <Section title="自分のレート"
               aside={<Button size="chip" onClick={() => {
                 for (const t of ['8', '8r']) void ggsApi.rank(t, snap.login);
               }}>更新</Button>}>
        {!mine.length && <Empty>まだ記録がありません。</Empty>}
        {mine.map((r) => (
          <RateRow key={r.gtype} label={gtypeLabel(r.gtype)}
                   rate={{ value: r.rating, dev: r.dev, rank: r.rank, w: r.wins, l: r.losses, d: r.draws }} />
        ))}
      </Section>

      <Section title={mode === 'who' ? '接続中' : 'ランキング'}
               aside={<span style={{ display: 'flex', gap: 'var(--sp-2)', alignItems: 'center' }}>
                 <Segmented size="chip" value={mode} onChange={(m) => {
                   setMode(m);
                   if (m === 'who') void ggsApi.who('8'); else void ggsApi.top('8', 100);
                 }} options={[{ value: 'who', label: '接続中' }, { value: 'top', label: '上位' }]} />
               </span>}>
        {!rows.length && <Empty>いません。</Empty>}
        {rows.map((u) => (
          <button key={u.name} type="button" className="k-row" onClick={() => setSel(u.name)}
            style={{
              width: '100%', border: 0, background: 'transparent', textAlign: 'left',
              display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
              height: 'var(--h-field)', padding: '0 var(--sp-2)', fontSize: 'var(--fs-5)',
              borderBottom: '1px solid var(--border-weak)',
            }}>
            <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>{u.name}</span>
            {u.rating != null && (
              <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>{u.rating.toFixed(1)}</span>
            )}
            {snap.ongoing.some((o) => o.names.includes(u.name)) && <Tag tone="ok">対局中</Tag>}
          </button>
        ))}
      </Section>
    </div>
  );
}

function UserDetail({ snap, name, tab, onTab, onBack, onNav }: {
  snap: GgsSnapshot; name: string; tab: string;
  onTab: (t: string) => void; onBack: () => void; onNav: (id: NavId) => void;
}) {
  useEffect(() => { ggsApi.finger(name).catch(() => {}); }, [name]);
  useEffect(() => { ggsApi.history(name === snap.login ? '' : name).catch(() => {}); }, [name, snap.login]);

  const u = snap.users.find((x) => x.name === name);
  // finger の生の行から "レート@偏差" を拾う (一覧の数字より詳しい)
  const m = /(\d+(?:\.\d+)?)@\s*(\d+(?:\.\d+)?)/.exec(u?.raw || '');
  const rate = m ? m[1] : (u?.rating != null ? u.rating.toFixed(1) : '');
  const dev = m ? Math.round(parseFloat(m[2])) : null;
  const playing = snap.ongoing.some((o) => o.names.includes(name));
  const fields = snap.fingers[name]?.fields ?? [];
  const rows = snap.history[name === snap.login ? '' : name] ?? [];

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      {/* 頭は帯。名前・レート・対局の状況・申し込みを 1 行に収める */}
      <div style={{
        flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        padding: 'var(--sp-3) var(--sp-4)', borderBottom: '1px solid var(--border)',
        background: 'var(--panel)',
      }}>
        <IconButton name="back" label="一覧へ戻る" onClick={onBack} />
        <span style={{ fontSize: 'var(--fs-2)', fontWeight: 600 }}>{name}</span>
        {rate && (
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
            {rate}{dev != null && <span style={{ opacity: .7, marginLeft: 4 }}>±{dev}</span>}
          </span>
        )}
        {playing && <Tag tone="ok">対局中</Tag>}
        <span style={{ marginLeft: 'auto' }} />
        <Button variant="primary" onClick={() => onNav('ggs-lobby')}>対局を申し込む</Button>
      </div>
      <div style={{ flex: 'none', padding: 'var(--sp-2) var(--sp-4)' }}>
        <Segmented value={tab} onChange={onTab}
                   options={[{ value: 'プロフィール', label: 'プロフィール' },
                             { value: '対戦履歴', label: '対戦履歴' }]} />
      </div>

      <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: '0 var(--sp-4) var(--sp-4)' }}>
        {tab === 'プロフィール' ? (
          <>
            {!fields.length && <Empty>読み込んでいます…</Empty>}
            {fields.map(([k, v]) => {
              const key = normKey(k).replace(/\(.*\)/, '');
              // 条件式は文字に潰さず木のまま描く。構造がそのまま意味になっている
              const cond = ['accept', 'decline', 'request'].includes(key) ? parseCond(v) : null;
              return (
                <div key={k} style={{
                  display: 'flex', gap: 'var(--sp-3)', alignItems: 'flex-start',
                  padding: '6px 0', borderBottom: '1px solid var(--border-weak)',
                }}>
                  <span style={{
                    width: 'var(--w-label)', flex: 'none', fontSize: 'var(--fs-6)', color: 'var(--sub)',
                  }}>{k}</span>
                  <span style={{ flex: 1, minWidth: 0, fontSize: 'var(--fs-5)' }}>
                    {cond ? <FormulaView node={cond} top />
                      : ['accept', 'decline', 'request'].includes(key) ? '指定なし'
                      : fingerValue(k, v)}
                  </span>
                </div>
              );
            })}
          </>
        ) : (
          <>
            {!rows.length && <Empty>対戦履歴はありません。</Empty>}
            {rows.map((h) => (
              <Row key={h.id}
                   title={`${h.black} 対 ${h.white}`}
                   sub={`${h.at} · ${gtypeLabel(h.gtype)} · ${h.score}`} />
            ))}
          </>
        )}
      </div>
    </div>
  );
}
