import React, { useEffect, useRef, useState } from 'react';
import { api, ggsApi, jsLog, onApp } from './api';
import type { ChatMsg, GameResult, GgsSnapshot, MatchView, UserRow } from './types';
import {
  CLOCK_CHOICES, GTYPE_CHOICES, clockOf, countDiscs, ggsMoveToIndex, gtypeLabel,
  fingerGroups, fingerValue, hasJapanese, normKey, parseCond, translate, useClocks,
  type ClockSide, type ClockView,
} from './ggs';
import { Col, Empty, EmptyBoard, EmptyState, List, Modal, Note, Overlay, Section, TableHead, TableRow, picked } from './components/layout';
import { Button, Segmented, Select, TextField, Toggle } from './components/primitives';
import { Strength } from './components/strength';
import { Confirm, PickOne } from './Dialogs';
import { IconButton } from './components/Icons';
import {
  Bubble, ConsoleLog, DayMark, FormulaEditor, FormulaView, MatchRow, PlayerRow, RateRow, Tag,
  type Cond, type Match, type NavId,
} from './components/ggs';
import { Board, type Cell } from './components/board';
import { RateChart, ResultRow, StoneDot } from './components/data';
import { flipped, type Prefs } from './prefs';
import { logLinesOf } from './adapt';

/* GGS の画面。左メニューの行き先ごとに中身を出し分ける。
 *
 * 未接続のときは 7 行を出さずログイン 1 行だけになるので、ここへ来るのは
 * ログイン画面だけ。残りは順に置き換えていく。
 */

export function GgsScreen({ nav, snap, onNav, prefs, onKifu }: {
  nav: NavId; snap: GgsSnapshot | null; onNav: (id: NavId) => void; prefs: Prefs;
  /** 棋譜を覆いで見せる。手元に棋譜が無いときは `archive` から取り出す。 */
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  if (nav === 'ggs-login') return <GgsLogin />;
  if (!snap) return <EmptyState title="GGS に接続していません" />;

  switch (nav) {
    case 'ggs-play': return <GgsPlay snap={snap} onNav={onNav} prefs={prefs} onKifu={onKifu} />;
    case 'ggs-lobby': return <GgsLobby snap={snap} onNav={onNav} />;
    case 'ggs-players': return <GgsUsers snap={snap} onNav={onNav} onKifu={onKifu} />;
    case 'ggs-results': return <GgsResults snap={snap} onKifu={onKifu} />;
    case 'ggs-chat': return <GgsChat snap={snap} />;
    case 'ggs-standby': return <GgsStandby snap={snap} onNav={onNav} />;
    case 'ggs-console': return <GgsConsole snap={snap} />;
    case 'ggs-settings': return <GgsSettings snap={snap} />;
    default: return null;
  }
}

/* 浮く箱の中の入力ラベル。節 (1px 罫つきの見出し) はコンテンツ領域のもので、
 * 340px の箱の中に置くと罫が箱を横切って見出しに見えてしまう */
/* 縦並びの中では、`alignItems` を `flex-start` にすると中身が自分の幅まで
 * 縮む (TextField の `flex:1` は縦並びでは高さの話になる)。選択肢の幅を
 * 内容で決めたい欄はそれでよいが、**打ち込む欄は箱いっぱいに伸ばす** —
 * 半分の幅で止まっている入力欄は、押せる場所に見えない。 */
function Field({ label, children, stretch }: { label: string; children: React.ReactNode; stretch?: boolean }) {
  return (
    /* `alignSelf: 'start'` が要る。**格子の子は既定で行の高さまで伸びる**
       ので、同じ行に背の高いもの (説明文つきの「レート戦」など) があると
       この器も伸び、中の入力欄が `flexGrow` で余りを吸って縦長になる。
       実際に「最大対局数」と「対局の間隔」が 3 倍の高さになっていた。 */
    <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
                    alignSelf: 'start',
                    alignItems: stretch ? 'stretch' : 'flex-start' }}>
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
      {/* 設計の実測: 箱 340px / 角丸 --r-4 / 地は --panel / 余白 22px /
          行間 14px。欄は 32px で地は --bg。**釦は箱いっぱいの幅**で、
          状態の文はその下に置く (欄と釦が縦に一続きになる)。
          見出しは節と同じ小さい字 (10px・600・字間 .08em)。 */}
      <div style={{
        width: 'var(--w-modal)', borderRadius: 'var(--r-4)', background: 'var(--panel)',
        border: '1px solid var(--border)', padding: 22,
        display: 'flex', flexDirection: 'column', gap: 'var(--sp-3h)',
      }}>
        <div style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>GGS へログイン</div>
        <Note>
          <span style={{ fontFamily: 'var(--ff-mono)' }}>skatgame.net:5000</span>
          {' '}— ログインに成功するとキーチェーンに保存され、次回から自動ログインします
        </Note>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
          <LoginField label="ログイン名">
            <TextField value={user} onChange={setUser} />
          </LoginField>
          <LoginField label="パスワード">
            <TextField value={pw} password onChange={setPw} />
          </LoginField>
        </div>
        <Button size="field" variant="primary" className="k-wide"
                onClick={() => void connect()}>ログイン</Button>
        {/* 文が出ていなくても場所を空けておく — 出た瞬間に下がずれない */}
        <div style={{
          fontSize: 'var(--fs-6)', minHeight: 16,
          color: status.startsWith('接続') ? 'var(--sub)' : 'var(--bad)',
        }}>{status}</div>
      </div>
    </div>
  );
}

/** ログインの欄。見出しは節と同じ小さい字 (設計の `.fld`)。 */
function LoginField({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 5, alignItems: 'stretch' }}>
      <span style={{
        fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)',
      }}>{label}</span>
      {children}
    </label>
  );
}

/* ---------------- コンソール ----------------
 *
 * 通信ログと生コマンド。GGS は画面に出していない機能が多いので、
 * 逃げ道として直に打てる場所を必ず残す。
 */
export function GgsConsole({ snap }: { snap: GgsSnapshot }) {
  const [cmd, setCmd] = useState('');
  /* 絞り込みと「クリア」。設計 §6 の見出しに載っている。
     **クリアは画面の話** — サーバーとのやりとりを消してしまうのではなく、
     ここから先だけを見る印を置く (端末の clear と同じ)。数で覚えるので
     ログが伸びるぶんには狂わない */
  const [dir, setDir] = useState<'all' | 'out' | 'in'>('all');
  const [from, setFrom] = useState(0);
  /* 押した結果の一言 (規則 34 — 出すのは失敗と、押したのに進まない理由) */
  const [note, setNote] = useState('');
  const say = (t: string) => { setNote(t); window.setTimeout(() => setNote(''), 2500); };
  const send = () => {
    const c = cmd.trim();
    if (!c) return;
    void ggsApi.raw(c);
    setCmd('');
  };
  const all = logLinesOf(snap.log);
  const shown = all.slice(Math.min(from, all.length))
    // 「送信」「受信」は自分が打ったものと相手から来たもの。app (アプリの
    // 断り書き) はどちらでもないので、絞ったときは落とす
    .filter((l) => dir === 'all' || l.dir === dir);
  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', padding: '0 var(--sp-4)' }}>
        <Section title="通信ログ" aside={<>
          <Segmented value={dir} onChange={setDir} options={[
            { value: 'all', label: 'すべて' },
            { value: 'out', label: '送信' },
            { value: 'in', label: '受信' },
          ]} />
          <Button onClick={() => setFrom(all.length)} disabled={!all.length}>クリア</Button>
          <Button disabled={!shown.length}
                  onClick={() => void ggsApi.saveLog(
                    shown.map((l) => (l.dir === 'out' ? '› ' : '') + l.text).join('\n') + '\n',
                  )
                    // 成功したときは何も言わない (規則 34)
                    .catch((e) => say('保存できませんでした (' + e + ')'))}>保存</Button>
          {note && <span style={{
            fontSize: 'var(--fs-6)', letterSpacing: 0, color: 'var(--bad)',
          }}>{note}</span>}
        </>} />
        <ConsoleLog lines={shown} />
      </div>
      <div style={{
        flex: 'none', display: 'flex', gap: 'var(--sp-2)', alignItems: 'center',
        padding: 'var(--sp-3) var(--sp-4)', borderTop: '1px solid var(--border-weak)',
      }}>
        {/* コンソールも Enter で送れる。生コマンドを何度も打つ場所なので、
            毎回釦まで手を運ばせない */}
        <TextField mono value={cmd} onChange={setCmd} onEnter={send}
                   placeholder="コマンド (例: tell /os who 8。Enter で送信)" />
        <Button size="field" onClick={send}>送信</Button>
      </div>
    </div>
  );
}

/* ---------------- チャット ----------------
 *
 * 左に会話の一覧 (全体チャット + 話した相手ごと)、右に選んだ会話。
 * 英語の発言は自動で和訳を添え、日本語の発言は英訳して送れる。
 */

/** ここより偏差が大きいレートは**まだ動く** = 暫定として印を付ける。
 *
 * **GGS は「暫定」を直接は返さない。**`rank` の生の行 (`2184.2@180.8=`) に
 * 印らしき字はあるが、暫定のはずの 2 つ (6 局と 5 局) がどちらも `=` で、
 * これでは見分けが付かなかった。**偏差で決める**しかない。
 *
 * 100 にした根拠 — 絵 (§5) が「1795.1±112」を**暫定**として描いている。
 * 実データ (接続中の一覧) も、初めての人が ±350 (初期値)、数局の人が
 * ±125〜216、よく打つ常連が ±44 / ±75 / ±91 と割れている。
 * **要確認** (絵の例 1 つを根拠にしているので、閾値は相談したい)。 */
const PROVISIONAL_DEV = 100;

/** 訳文を添える対象か (自分以外の英語の発言だけ)。 */
const wantsTranslation = (c: ChatMsg, login: string): boolean =>
  c.from !== login && !hasJapanese(c.text) && /[a-zA-Z]{2,}/.test(c.text);

const trKey = (c: ChatMsg): string => c.from + '|' + c.text;

export function GgsChat({ snap }: { snap: GgsSnapshot }) {
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
      <ChatList sorted={sorted} cur={cur} onThread={setThread} onPick={setPick} />

      <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
        {/* どの会話を見ているかと、**誰に届くか**を頭に出す。左の一覧の
            選ばれている行だけでは、送る直前に確かめる場所が無い */}
        <div style={{
          flex: 'none', display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)',
          padding: 'var(--sp-3) var(--sp-4)', borderBottom: '1px solid var(--border-weak)',
        }}>
          <span style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>
            {cur === '.chat' ? '全体チャット' : cur}
          </span>
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
            {cur === '.chat' ? 'ここにいる全員に届きます' : '本人にだけ届きます'}
          </span>
        </div>
        {/* 訳の切り替えは**送る行から出して独立した帯にする** (絵もこの位置)。
            入力欄と同じ行に置くと、打つ場所が 2 つのつまみに押されて狭くなる。
            どちらも「これから送る 1 通」ではなく**この会話の見え方**の設定
            なので、上に置くほうが筋も合う */}
        <div style={{
          flex: 'none', height: 'var(--h-field)', display: 'flex', alignItems: 'center',
          gap: 'var(--sp-4)', padding: '0 var(--sp-4)', borderBottom: '1px solid var(--border-weak)',
        }}>
          <Toggle checked={autoJa} onChange={setAutoJa} label="和訳して表示" />
          <Toggle checked={toEn} onChange={setToEn} label="英訳して送信" />
        </div>
        <div className="k-scroll" ref={box} style={{
          flex: 1, minHeight: 0, padding: 'var(--sp-4)',
          display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)',
        }}>
          {/* 何も無いと壊れているのか、単に発言が無いのかが分からない。
              全体チャットは静かなことが多いので、ここは必ず埋める */}
          {/* 発言は下から積むので、空のときだけ真ん中へ置く */}
          {!rows.length && (
            <span style={{ margin: 'auto' }}>
              <Empty>{cur === '.chat' ? 'まだ発言はありません。' : 'まだやりとりはありません。'}</Empty>
            </span>
          )}
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
          <TextField value={text} onChange={setText} onEnter={() => void send()}
                     placeholder="メッセージを入力 (Enter で送信)" />
          <Button size="field" variant="primary" onClick={() => void send()}>送信</Button>
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
 * 左が一覧、右が「これから始める」もの。
 *
 * **右の列は --w-dock (290px)。** 設計には --w-lobby (174px) というトークンが
 * あるが使っていない — 174px では「同期・ランダム16手 (推奨)」の選択が
 * 折り返すし、画面ごとに違う幅を作ると行き来したときに本体の幅が動く。
 */
function GgsLobby({ snap, onNav }: { snap: GgsSnapshot; onNav: (id: NavId) => void }) {
  const [opp, setOpp] = useState('');
  const [gtype, setGtype] = useState('s8r16');
  const [time, setTime] = useState('00:15:00');
  const noRated = useNoRated();
  const calibrated = useCalibrated();
  const [rated, setRated] = useState(true);
  /** 「情報」を開いている申し込みの id (1 つだけ)。 */
  const [info, setInfo] = useState('');
  /* 「対局中」の節も進行中の一覧から出す。**届くのはログイン時と 60 秒ごと**
     なので、開いた時点で聞き直す (プレイヤーの一覧と同じ理由)。 */
  useEffect(() => { void ggsApi.listMatches().catch(() => {}); }, []);

  const games = snap.ongoing.filter((o) => !o.mine);
  const names = snap.users.filter((u) => u.name !== snap.login).map((u) => u.name);

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <div className="k-scroll" style={{ flex: 1, minWidth: 0, padding: 'var(--sp-4) var(--sp-2) 0' }}>
        <Section title="対局中" aside={games.length ? `${games.length} 局` : undefined}>
          {!games.length && <Empty>進行中の対局はありません。</Empty>}
          {/* 行どうしは詰める。節の余白 (--sp-3 = 12px) が行間に入ると、
              一覧ではなく箇条書きに見える (定石の木・学習ログでも踏んだ) */}
          <List>
          {games.map((o) => (
            <Row key={o.id}
                 title={`${o.names[0] || '?'} 対 ${o.names[1] || '?'}`}
                 sub={gtypeLabel(o.gtype)}
                 actions={
                   <Button size="row" variant={o.watching ? 'danger' : 'primary'}
                           onClick={() => {
                             const on = !o.watching;
                             void ggsApi.watch(o.id, on);
                             // 観戦を始めたら盤が見たいはずなので対局画面へ移る。
                             // **その対局**を映す (先頭の別の対局ではなく)
                             if (on) { focusMatch(o.id); onNav('ggs-play'); }
                           }}>
                     {o.watching ? '観戦をやめる' : '観戦'}
                   </Button>} />
          ))}
          </List>
        </Section>

        <Section title="対局の申し込み">
          {!snap.offers.length && <Empty>対局の申し込みはありません。</Empty>}
          {/* **ここも `List` で包む。**節の余白 (12px) が行間に入ると一覧に
              見えない — 対局中・定石の木・学習ログ・GGS の 3 つの一覧で
              同じ罠を踏んでいて、これが 5 か所目だった */}
          <List>
          {snap.offers.map((o) => {
            const who = o.names.filter((n) => n !== snap.login);
            return (
              <React.Fragment key={o.id}>
              <Row title={who.join(' と ') || '?'}
                   // 規則 27 — 自分宛と未読だけ --bad で塗る。accent にすると
                   // 「押せる場所」の青と同じになり、急ぎのものが埋もれる
                   tag={o.incoming ? '自分宛' : undefined}
                   tagTone={o.incoming ? 'bad' : undefined}
                   alert={o.incoming}
                   sub={`${gtypeLabel(o.gtype)} · ${o.time || '?'}${o.rated ? ' · レート戦' : ''}`}
                   actions={<>
                     {/* 絵は自分宛でない申し込みにも「情報」を置いている。
                         **まとめた 1 行は色・コミ・乱数の手数を落としている**ので、
                         受けるかどうかを決める前に元の行を読めるようにする */}
                     <Button size="row" onClick={() => setInfo(info === o.id ? '' : o.id)}>情報</Button>
                     {o.incoming && <>
                       <Button size="row" variant="primary"
                               onClick={() => { focusMatch(o.id); void ggsApi.accept(o.id); }}>受ける</Button>
                       <Button size="row" variant="danger" onClick={() => void ggsApi.decline(o.id)}>断る</Button>
                     </>}
                   </>} />
              {info === o.id && (
                <div style={{
                  padding: 'var(--sp-2) var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
                  fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-7)', color: 'var(--sub)',
                  lineHeight: 1.7, wordBreak: 'break-all',
                }}>{o.raw || '元の行がありません'}</div>
              )}
              </React.Fragment>
            );
          })}
          </List>
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
          {/* GGS の /os ではレート有無はアカウント単位の設定なので、
              申し込みの直前に毎回送って揃える (バックエンド側)。ここは
              「この申し込みをどちらにするか」だけを持つ */}
          {/* 見出しは名詞、駒は動詞 — 「座標: 出す / 出さない」「定石: 使う /
              使わない」と同じ形に揃える。**「レート戦」は GGS で通じている
              呼び名**なので、そのまま見出しにするのがいちばん短く読める
              (「レートに反映」だと言い換えになり、駒との対も冗長になる) */}
          <Field label="レート戦">
            {/* **禁じられているときは押せなくして理由をその場に出す**
                (規則 61)。送る側でも潰しているので、ここは見た目の話 */}
            <Segmented value={rated && !noRated ? 'on' : 'off'} disabled={noRated}
                       onChange={(v) => setRated(v === 'on')}
                       options={[{ value: 'on', label: 'する' },
                                 { value: 'off', label: 'しない' }]} />
            {noRated && <Note>レート戦を禁じて起動しています (KUROOBI_NO_RATED)。</Note>}
          </Field>
          {/* 相手を指定しない申し込みは「誰でも受けられる」募集になる。
              GGS の /os ask はそういう使い方ができるので、止めない */}
          {/* 上の欄と同じ 32px。欄を埋めて押す釦なので、欄より低いと
              一続きの手順に見えない (ログインの釦と同じ理由) */}
          <Button size="field" variant="primary" disabled={!calibrated}
                  onClick={() => void ggsApi.ask(gtype, time, opp, rated)}>
            {opp ? '申し込む' : '募集する'}
          </Button>
          {!calibrated && <Note>{CALIB_NOTE}</Note>}
          <Note>
            同期対局は同じ開局を先後入れ替えて 2 局同時に行い、結果は合計で判定します。
            レートは「ランダム開局」に反映されます。
          </Note>
        </Section>

        {/* 中断対局は login のときに 1 度だけ流れてくる。あとから相手が
            中断したぶんは、こちらから聞き直さないと出てこない */}
        <Section title="中断対局"
                 aside={<Button onClick={() => void ggsApi.listStored()}>更新</Button>}>
          {!snap.stored.length && <Empty>中断対局はありません。</Empty>}
          {/* 行どうしは詰める (節の余白 12px が行間に入ると一覧に見えない) */}
          <List>
          {snap.stored.map((x) => (
            <Row key={x.id} title={x.opp || '?'} sub={gtypeLabel(x.gtype)}
                 actions={<Button size="row" variant="primary"
                                  onClick={() => void ggsApi.resumeStored(x.id)}>再開</Button>} />
          ))}
          </List>
        </Section>
      </aside>
    </div>
  );
}

/** 一覧の 1 行。名前と補足が左、操作が右。行そのものは押さない */
function Row({ title, sub, tag, tagTone, alert, actions, onClick, title2 }: {
  title: string; sub?: string; tag?: string;
  /** 印の色。既定は accent (規則 27 — 自分宛と未読だけ --bad)。 */
  tagTone?: 'sub' | 'accent' | 'ok' | 'bad';
  /** 自分宛。左に --bad の帯を引く (設計 §4)。 */
  alert?: boolean;
  actions?: React.ReactNode;
  /** 押せる行にする。行の中に別の押せるものがあるときは使わない (規則 46)。 */
  onClick?: () => void;
  /** 押せる行の補足 (title 属性)。 */
  title2?: string;
}) {
  /* 押せるなら押せる要素で書く (規則 41)。div + onClick だと Tab で
     回ってこないし Enter / Space でも選べない。 */
  const Tag_ = onClick && !actions ? 'button' : 'div';
  return (
    <Tag_ {...(onClick && !actions
      ? { type: 'button' as const, className: 'k-row', onClick, title: title2 }
      : {})} style={{
      display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', width: '100%',
      /* **高さは固定** (絵の実測 40)。中身で伸ばすと、補足のある行と
         ない行で高さが変わって一覧が揃わない。24px の行とは別の型 */
      height: 'var(--h-row2)', flex: 'none', padding: '0 var(--sp-4)',
      // 一括の border を先に置く。**あとに書くと borderBottom を消す** —
      // 行の区切りが全部消えるが、GGS に繋がないと見えないので気付けない
      border: 0, borderRadius: 0,
      borderBottom: '1px solid var(--border-weak)',
      /* 自分宛の印 (設計 §4)。**罫ではなく内側の影と薄い地** — 選んでいる
         行 (`picked`) と同じ形で、罫にすると中身が 3px ずれる。色は規則 27 */
      background: alert ? 'color-mix(in srgb, var(--bad) 8%, transparent)' : 'transparent',
      boxShadow: alert ? 'inset 2px 0 0 var(--bad)' : undefined,
      textAlign: 'left',
      color: 'var(--text)', cursor: onClick && !actions ? 'pointer' : undefined,
    }}>
      {/* 2 段の溝は 2px。sp-1 (4) だと 40px に収まらない (絵の実測) */}
      <span style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 2 }}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-5)' }}>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{title}</span>
          {tag && <Tag tone={tagTone ?? 'accent'}>{tag}</Tag>}
        </span>
        {sub && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{sub}</span>}
      </span>
      {actions}
    </Tag_>
  );
}

/** レート戦が禁じられているか。**自動で回すときの蓋** (`KUROOBI_NO_RATED=1`)。
 *  送る側でも潰しているので、これは「選べないことを見せる」ための値。 */
function useNoRated(): boolean {
  const [no, setNo] = useState(false);
  useEffect(() => { ggsApi.noRated().then(setNo).catch(() => {}); }, []);
  return no;
}

/** 読切の速度を測ってあるか。
 *
 * **測っていないと持ち時間の管理が固定の階段に落ちる。** 機械の速さを
 * 知らないまま対局に入ることになり、GGS では時間切れがレートに直結する。
 * だから時間の絡む操作 (申し込み・待ち受け・持ち時間の設定) はここで
 * 止める。**バックエンドでも同じ判定をしている** — 画面だけで止めると、
 * 古い画面や別経路から通ってしまう。
 *
 * 既定は「測ってある」。取りに行く前の一瞬だけ押せなくなるのを避ける
 * (通っても後ろで止まる)。
 */
function useCalibrated(): boolean {
  const [ok, setOk] = useState(true);
  useEffect(() => {
    let alive = true;
    const load = () => void api.localThreads()
      .then((t) => { if (alive) setOk(t.nps != null); })
      .catch(() => {});
    load();
    // 起動時の較正は背景で走る。終わったら報せが来る
    const off = onApp('resources-changed', load);
    return () => { alive = false; void off.then((f) => f()); };
  }, []);
  return ok;
}

/** 未較正のときに出す一言。**直し方まで書く** (規則 34)。 */
const CALIB_NOTE = '読切の速度をまだ測っていません。設定 → エンジン の「読切の速度」で測ってください (数秒で終わります)。';

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
  const noRated = useNoRated();
  const calibrated = useCalibrated();
  const [rated, setRated] = useState(sb.rated);

  const names = snap.users.filter((u) => u.name !== snap.login).map((u) => u.name);
  /* **終局した対局は数えない。** 棋譜を見られるよう一覧には残す作りなので、
     そのまま数えると一度対局しただけで「対局中」から戻らなくなる
     (バックエンドの自動受諾も同じ判定を使っている) */
  const playing = snap.matches.some((m) => !m.over);
  const state = sb.enabled ? (playing ? '対局中' : '申し込み待ち') : '停止中';

  const toggle = () => void ggsApi.setStandby({
    // **禁じられているときは何が来ても false。** 画面が古くても通さない
    enabled: !sb.enabled, auto_accept: autoAccept, rated: rated && !noRated, opponent: opp.trim(),
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
        {/* 設計は 3 列の格子。**幅を欄ごとに決めない** — 決めると
            「同期・ランダム16手 (推奨)」だけが窮屈になり、数値欄だけが
            極端に短くなって列が揃わない。器の幅を 3 等分して各欄に配る */}
        <div style={{
          display: 'grid', gap: 'var(--sp-4)',
          gridTemplateColumns: 'repeat(auto-fit, minmax(240px, 1fr))',
        }}>
          <Field stretch label="相手">
            <Select value={names.includes(opp) ? opp : ''} onChange={setOpp}
                    options={[['', '指定しない (誰でも)'], ...names.map((n) => [n, n] as [string, string])]} />
          </Field>
          <Field stretch label="形式"><Select value={gtype} onChange={setGtype} options={GTYPE_CHOICES} /></Field>
          <Field stretch label="持ち時間"><Select value={time} onChange={setTime} options={CLOCK_CHOICES} /></Field>
          <Field stretch label="最大対局数 (0 = 無制限)">
            <TextField numeric align="right" value={String(maxGames)}
                       onChange={(x) => setMaxGames(+x || 0)} />
          </Field>
          <Field stretch label="対局の間隔 (秒)">
            <TextField numeric align="right" value={String(interval)}
                       onChange={(x) => setInterval(+x || 0)} />
          </Field>
          {/* こちらから申し込むときのレート有無。GGS ではアカウント単位の
              設定なので、申し込みの直前に毎回送って揃える (バックエンド側)。
              受ける側のレート有無は申し込んだ相手が決めるので変えられない */}
          <Field label="レート戦">
            {/* **禁じられているときは押せなくして理由をその場に出す**
                (規則 61)。送る側でも潰しているので、ここは見た目の話 */}
            <Segmented value={rated && !noRated ? 'on' : 'off'} disabled={noRated}
                       onChange={(v) => setRated(v === 'on')}
                       options={[{ value: 'on', label: 'する' },
                                 { value: 'off', label: 'しない' }]} />
            {noRated && <Note>レート戦を禁じて起動しています (KUROOBI_NO_RATED)。</Note>}
          </Field>
        </div>
        {/* Toggle は摘みを右端へ寄せる作りなので、幅を決めずに置くと画面の端まで離れる */}
        <span style={{ width: 300, display: 'block' }}>
          <Toggle checked={autoAccept} onChange={setAutoAccept} label="届いた申し込みを自動で受ける" />
        </span>
        <div>
          {/* **止めるのは未較正でも通す。** 身動きが取れなくなるほうが困る */}
          <Button variant={sb.enabled ? 'danger' : 'primary'}
                  disabled={!sb.enabled && !calibrated} onClick={toggle}>
            {sb.enabled ? '待機モードを停止' : '待機モードを開始'}
          </Button>
          {!sb.enabled && !calibrated && <Note>{CALIB_NOTE}</Note>}
        </div>
        <Note>
          対局終了 → 間隔待ち → 自動申し込みを繰り返します。切断時は自動で再接続し、
          中断対局も自動再開します。相手を指定しないときは自分からは申し込まず、
          届いた申し込みを受けるだけになります。
        </Note>
      </Section>

      {/* 節の名前と説明文は設計 §7 のまま。実装が書いていた「同じ設定を
          2 か所で編集できると、どちらが本物か分からなくなります」は
          **作る側の理屈**で、規則 64 が画面の器の中に入れるなと決めている
          (絵でも器の外の注記に置かれている)。画面に出すのは、読む人にとっての
          意味 — アプリを閉じても効くこと */}
      <Section title="申し込みの条件 (サーバー側)"
               aside={<Button onClick={() => onNav('ggs-settings')}>条件を変える</Button>}>
        <Note>
          アプリを閉じてもサーバー側で有効な条件です。待機モードの保険になります。
        </Note>
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

/* **明示的に開いた対局を最初に映す。**
 *
 * 一覧の先頭を既定にしていたので、観戦を始めても・対局が成立しても、
 * 別の対局が映ったままだった。押した本人はその対局が見たいので、押した
 * ところで id を控えて、対局画面が立ち上がるときに拾う。
 *
 * 画面をまたぐが、React の状態にするには App まで持ち上げることになる。
 * **一度きりの受け渡し**なので、置き場だけ用意して読んだら捨てる。 */
let wantedMatch = '';
export function focusMatch(id: string) { wantedMatch = id; }

/* ---------------- 対局・観戦 ----------------
 *
 * 同期対局は 2 局で 1 組。組を 1 行として扱い、右に盤を並べる。
 * 待った (undo) と中止 (abort) は出さない — どちらも相手の承諾が要る要求で、
 * GGS の相手はたいていプログラムなので通らない。投了だけは自分で決められる。
 */
function GgsPlay({ snap, onNav, prefs, onKifu }: {
  snap: GgsSnapshot; onNav: (id: NavId) => void; prefs: Prefs;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  /* 押して来たならその対局を映す。**読んだら捨てる** — 次に来たときまで
     残っていると、選び直した対局が勝手に戻る */
  const [sel, setSel] = useState(() => { const w = wantedMatch; wantedMatch = ''; return w; });
  const clock = useClocks(snap.matches);

  // 手合いにまとめる。自分の対局を先に、次に観戦
  const groups = new Map<string, MatchView[]>();
  for (const m of snap.matches) groups.set(m.base, [...(groups.get(m.base) ?? []), m]);
  /* **新しいものが上。** id は `.48` のような連番なので、数として比べる
     (文字列だと `.100` が `.48` より前に来る)。番号を持たないものは
     後ろへ回す。 */
  const num = (k: string) => {
    const n = parseInt(k.replace(/^\./, ''), 10);
    return Number.isFinite(n) ? n : -1;
  };
  const keys = [...groups.keys()].sort((a, b) => {
    const mine = (k: string) => (groups.get(k)!.some((m) => m.my_color) ? 0 : 1);
    return mine(a) - mine(b) || num(b) - num(a) || b.localeCompare(a);
  });
  const cur = groups.has(sel) ? sel : keys[0] ?? '';
  const pair = groups.get(cur);

  if (!groups.size) {
    return (
      <EmptyState title="対局はまだありません"
                  visual={<EmptyBoard />}
                  body="ロビーで申し込むか、進行中の対局を観戦できます。"
                  actions={<>
                    <Button variant="primary" onClick={() => onNav('ggs-lobby')}>ロビーへ</Button>
                    {/* 絵の 2 つ目の釦。対局が無いなら自動で申し込ませる方が早い */}
                    <Button onClick={() => onNav('ggs-standby')}>待機モードへ</Button>
                    {/* 繋ぎ直した直後などは一覧が古いことがある。聞き直す道を残す */}
                    <Button onClick={() => void ggsApi.listMatches()}>更新</Button>
                  </>} />
    );
  }

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <aside className="k-scroll" style={{
        width: 'var(--w-list)', flex: 'none', borderRight: '1px solid var(--border)', minHeight: 0,
      }}>
        {keys.map((key) => (
          <MatchRow key={key} m={matchRowOf(groups.get(key)!, key)}
                    active={key === cur} onSelect={() => setSel(key)}
                    // 終わった対局は一覧に残り続けていた。閉じる先は
                    // 「結果」に控えてあるので、ここから消しても失われない
                    onClose={() => {
                      void ggsApi.closeMatch(key);
                      if (key === sel) setSel('');
                    }} />
        ))}
      </aside>

      <div className="k-scroll" style={{ flex: 1, minWidth: 0, minHeight: 0, padding: 'var(--sp-3)' }}>
        {/* 同期対局は 2 面。横に並べて、狭ければ折り返す */}
        <div style={{ display: 'flex', gap: 'var(--sp-3)', flexWrap: 'wrap', justifyContent: 'center' }}>
          {pair?.map((m, i) => (
            <MatchBoard key={m.id} snap={snap} m={m} clock={clock} prefs={prefs} onKifu={onKifu}
                        // 同期対局は 2 局で 1 組。**どちらの面で自分が何色か**が
                        // 分からないと、並んだ 2 枚を見分けられない (設計 §2)
                        face={(pair?.length ?? 1) > 1 ? i + 1 : undefined} />
          ))}
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
    // 中断は「終局・結果なし」に見せない。誰が抜けたかまで出す
    ended: g.map((x) => x.ended).find(Boolean) || undefined,
    leftBy: g.map((x) => x.left_by).find(Boolean) || undefined,
  };
}

function MatchBoard({ snap, m, clock, prefs, onKifu, face }: {
  snap: GgsSnapshot; m: MatchView; clock: (id: string, side: ClockSide) => ClockView; prefs: Prefs;
  onKifu: (title: string, kifu: string, archive?: string) => void;
  /** 同期対局の何面目か (1 起点)。1 局だけの手合いでは渡さない。 */
  face?: number;
}) {
  const [resign, setResign] = useState(false);
  const observer = !m.my_color;
  const { black, white } = countDiscs(m.cells);
  // 最後に打たれた石に印を付ける (パスは石を置かないので飛ばす)
  const last = [...m.moves].reverse().map(ggsMoveToIndex).find((x) => x !== null) ?? null;
  // 自分の側もレートを出す。対局中に見たいのは「この 2 人の力量差」
  const myRate = snap.my_ranks.find((r) => r.gtype === (m.gtype.includes('r') ? '8r' : '8'))?.rating;
  /* **誰から見た値かを言う。** エンジンが返すのは指した側 (=自分) から
     見た石差だが、数字だけを行に置くと黒からの値とも読める。同期対局は
     面ごとに自分の色が逆なので、なおさら符号の向きが分からない。 */
  const myEval = m.last_eval != null
    ? '自分 ' + (m.last_from_book ? '定石 ' : '') + (m.last_eval > 0 ? '+' : '')
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
    //
    // **幅だけで頭打ちにすると低い窓で溢れる。** 盤は正方形なので幅が
    // そのまま高さになり、860×560 では 8 行目と時計・投了が画面の外に
    // 出ていた。窓の高さからも上限をかける (280px は盤以外が使う分 —
    // ツールバー 44 + 下帯 28 + 面の帯と 2 人の行と釦で約 130 + 余白)
    <div style={{
      flex: '1 1 300px', minWidth: 260, maxWidth: 'min(460px, calc(100vh - 280px))',
      display: 'flex', flexDirection: 'column',
      // 絵は 1 面ずつ枠に入れる (地 --panel / 角丸 11 / 余白 12)。
      // 2 枚が地続きだと、どこまでが 1 局なのかが読めない
      background: 'var(--panel)', borderRadius: 'var(--r-4)', padding: 'var(--sp-3)',
    }}>
      {/* 同期対局のときだけ。**自分の色は面ごとに逆**になる */}
      {face !== undefined && (
        <div style={{
          height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
          fontSize: 'var(--fs-7)', color: 'var(--sub)',
        }}>
          <span>{face} 面目</span>
          {m.my_color && <span>自分が{m.my_color === 'black' ? '黒' : '白'}</span>}
        </div>
      )}
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
        {/* 観戦の解析は**黒視点**に直して持っている (ggs.rs)。どちらから
            見た値かを書かないと、盤の上下と合っているのかが分からない */}
        {observer && m.watch_eval != null && (
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            解析 <StoneDot color="b" />
            {m.watch_eval > 0 ? '+' : ''}{m.watch_eval.toFixed(1)}
            {m.watch_best ? ` (${m.watch_best})` : ''}
          </span>
        )}
        {snap.thinking === m.id && <span style={{ color: 'var(--accent)' }}>思考中</span>}
        {/* **中断は終局と別物。** 石差が付かないので結果は出さず、誰が
            抜けたかを出す。中止 (両者合意) も勝敗なしで終わる */}
        {m.ended === 'adjourned' && (
          <span style={{ color: 'var(--gold)' }}>
            中断{m.left_by ? ` · ${m.left_by} が退室` : ''}
          </span>
        )}
        {m.ended === 'aborted' && <span style={{ color: 'var(--sub)' }}>中止</span>}
        {m.ended === 'finished' && m.result && (
          <span style={{ color: 'var(--text)' }}>終局 {m.result}</span>
        )}
        <span style={{ marginLeft: 'auto' }} />
        {/* 終わった対局も一覧に残るので、そこから棋譜を取り出せる。
            旧 GUI にあった道を戻した (規則 71) */}
        <Button
                onClick={() => onKifu(m.opp_name ? `${m.opp_name} との対局` : '対局の棋譜',
                                      m.ggf || m.moves.join(''))}>棋譜</Button>
        {!observer && !m.over && (
          <Button variant="danger"
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
/** 通常 8 とランダム開局 8r のレートを並べて書く。プールを選んでいない
    画面 (名刺・詳細・接続中の一覧) は必ず両方出す。 */
function bothRates(u: UserRow | undefined): string {
  if (!u) return '';
  // finger の生の行から "レート@偏差" を拾う (一覧の数字より詳しい)
  const m = /(\d+(?:\.\d+)?)@\s*(\d+(?:\.\d+)?)/.exec(u.raw || '');
  const r8 = m ? m[1] : (u.rating != null ? u.rating.toFixed(1) : '');
  const d8 = m ? Math.round(parseFloat(m[2])) : null;
  const parts: string[] = [];
  if (r8) parts.push(`通常 ${r8}${d8 != null ? ` ±${d8}` : ''}`);
  if (u.rating_r != null) {
    parts.push(`ランダム ${u.rating_r.toFixed(1)}${u.dev_r != null ? ` ±${Math.round(u.dev_r)}` : ''}`);
  }
  return parts.join(' · ');
}

export function GgsSettings({ snap }: { snap: GgsSnapshot }) {
  const e = snap.engine;
  const calibrated = useCalibrated();
  /* 押した結果の一言。成功も失敗も同じ場所に出す */
  const [saved, setSaved] = useState('');
  const say = (t: string) => { setSaved(t); window.setTimeout(() => setSaved(''), 2500); };
  const [levels, setLevels] = useState({ depth: e.depth, solve: e.solve, band: e.band });
  // 全体設定から来る値 (この画面では変えられない。表示のみ)
  const threads = e.threads;
  const [ponder, setPonder] = useState(e.ponder);
  const [auto, setAuto] = useState(snap.auto_play);
  const [watch, setWatch] = useState(snap.watch_analysis);
  const [book, setBook] = useState(e.use_book);
  /* **強さの決め方は 2 択。** 持ち時間から毎手決め直すか、Lv のとおりに
     読むか。配り方 (`slow`/`even`/`fast`) の 3 択は別の軸で、そちらは
     測って `fast` に一本化した — 2 択まで一緒に畳んだのは誤り。 */
  const [pace, setPace] = useState(e.pace === 'depth' ? 'depth' : 'fast');
  const byClock = pace !== 'depth';
  const [maxMove, setMaxMove] = useState(e.max_move_secs);
  const [reserve, setReserve] = useState(e.reserve_secs);
  const [budgetUse, setBudgetUse] = useState(e.budget_use);
  const [cores, setCores] = useState(0);
  useEffect(() => { api.activity().then((a) => setCores(a.cores)).catch(() => {}); }, []);

  const online = snap.conn === 'online';
  // 申し込みの条件はサーバーが持っている。開いたときに取り直す
  const login = snap.login;
  useEffect(() => { if (login) ggsApi.finger(login).catch(() => {}); }, [login]);
  const myForm = (key: 'accept' | 'decline'): string =>
    snap.fingers[login]?.fields
      .find(([k]) => k.replace(/\s+/g, '').replace(/\(.*\)/, '') === key)?.[1] ?? '';
  const saveForm = async (kind: 'aform' | 'dform', expr: string) => {
    try {
      await ggsApi.setFormula(kind, expr);
    } catch (e) {
      say('条件を送れませんでした (' + e + ')');
      return;
    }
    // 送っただけで信じない。サーバーの値を取り直して画面に反映する
    if (login) ggsApi.finger(login).catch(() => {});
  };

  /* **失敗を握り潰して「反映しました」と言わない** (規則 34)。
     以前は 5 本すべて `.catch(() => {})` で捨てたうえで必ず成功を出して
     いたので、繋がっていなくても反映されたように見えた */
  const apply = async () => {
    try {
      await ggsApi.setEngine(levels.depth, levels.solve, levels.band, ponder);
      await ggsApi.setPacing(pace, maxMove, reserve, budgetUse);
      await ggsApi.setAutoPlay(auto);
      await ggsApi.setWatchAnalysis(watch);
      await ggsApi.setUseBook(book);
    } catch (e) {
      say('反映できませんでした (' + e + ')');
      return;
    }
    say('反映しました');
  };

  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4) var(--sp-4) 0' }}>
      <div style={{ maxWidth: 720, display: 'flex', flexDirection: 'column' }}>
        <Section title="強さ">
          <Field label="決め方">
            <Segmented value={pace} onChange={setPace}
                       options={[{ value: 'fast', label: '持ち時間で決める' },
                                 { value: 'depth', label: 'Lv で決める' }]} />
          </Field>
          <Note>
            {byClock
              ? '深さ・読切・選択読みを、残り時間から毎手決めます。'
              : '選んだ Lv のとおりに読みます。時間は見ません。'}
          </Note>
          {/* **時間で決めるなら目盛りは出さない。** 深さも読切も帯も時間から
              決まるので、置いても効かない物を並べることになる */}
          {!byClock && (
            <span style={{ maxWidth: 340, display: 'block' }}>
              <Strength value={levels} onChange={setLevels} />
            </span>
          )}
          {/* **スレッド数はここに置かない。** 設定 → エンジンの全体設定を
              GGS も使う。別々に持つと読切速度の較正 (機械ごとの実測) が
              2 つ要り、片方が未較正のまま対局に入って持ち時間の管理が
              固定の階段に落ちる */}
          <Field label="スレッド">
            <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>
              {threads === 0 ? `自動 (${Math.max(1, Math.floor(cores / 2)) || '—'})` : threads}
              {' · '}設定 → エンジンで変えられます
            </span>
          </Field>
          {/* 先読み。**「深さ固定」でも効く** — 効き方が「深く」から
              「速く」に変わるだけ (同じ深さへ 1/3 の時間で着く。実測) */}
          <Field label="先読み">
            <Segmented value={ponder ? 'on' : 'off'}
                       onChange={(v) => setPonder(v === 'on')}
                       options={[{ value: 'on', label: 'する' },
                                 { value: 'off', label: 'しない' }]} />
          </Field>
          <Note>相手の手番中に先を読みます。自分の持ち時間は減りません。</Note>
        </Section>

        {/* **配り方は選ばせない。** 4 択で測ったところ「じっくり」は 3 秒・
            8 秒の対局で勝率 0.0% (石差 −34)、「速指し」は全条件で「均等」に
            劣らなかった。正解と罠が並んでいるだけだったので一本化した。
            上の「決め方」はこれとは別の軸なので畳まない。 */}
        <Section title="持ち時間の使い方">
          {!byClock && (
            <Note>
              <b style={{ color: 'var(--text)' }}>「Lv で決める」を選んでいるので、ここは効きません。</b>
            </Note>
          )}
          <Note>1 手の時間を残り時間と残り手数から決めます。序盤を短く、終盤に残します。</Note>
          {(
            <div style={{ display: 'flex', gap: 'var(--sp-4)' }}>
              <Field label="1 手の上限 (秒、0 = なし)">
                <TextField numeric align="right" width={80} value={String(maxMove)}
                           onChange={(x) => setMaxMove(Math.max(0, +x || 0))} />
              </Field>
              <Field label="読切に残す (秒)">
                <TextField numeric align="right" width={80} value={String(reserve)}
                           onChange={(x) => setReserve(Math.max(0, +x || 0))} />
              </Field>
              <Field label="攻めて使う度合い">
                <TextField numeric align="right" width={80} value={String(budgetUse)}
                           onChange={(x) => setBudgetUse(Math.max(0.1, +x || 2.5))} />
              </Field>
            </div>
          )}
          <Note>「攻めて使う度合い」は 1.0 で配分どおり。大きいほど 1 手に長く使います。</Note>
        </Section>

        <Section title="定石">
          {e.book_loaded
            ? <div><Segmented value={book ? 'on' : 'off'} onChange={(v) => setBook(v === 'on')}
                              options={[{ value: 'on', label: '使う' }, { value: 'off', label: '使わない' }]} /></div>
            : <Note>ファイルがありません。左メニュー下の「設定」で指定してください。</Note>}
        </Section>

        {/* **ここだけはサーバーに置いてある値なので、繋がないと読めない。**
            上の 強さ / 持ち時間 / 定石 / ふるまい は手元の設定で、
            未接続でも `ggs.rs` の待ち受けが受け取る (1015〜1055 行) */}
        <Section title="申し込みの扱い">
          <Note>申し込みを自動で受ける / 断る条件です。サーバー側に残ります。</Note>
          {online ? <>
            <FormulaField label="自動で受ける条件" src={myForm('accept')}
                          onSave={(s) => void saveForm('aform', s)} />
            <FormulaField label="自動で断る条件" src={myForm('decline')}
                          onSave={(s) => void saveForm('dform', s)} />
          </> : (
            <Note>繋いでいないあいだは読めません。ログインすると出ます。</Note>
          )}
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
          {saved && (
            <span style={{
              fontSize: 'var(--fs-6)',
              color: saved.startsWith('反映しました') ? 'var(--ok)' : 'var(--bad)',
            }}>{saved}</span>
          )}
          <span style={{ marginLeft: 'auto' }} />
          <Button variant="primary" disabled={!calibrated}
                  onClick={() => void apply()}>適用</Button>
          {!calibrated && <Note>{CALIB_NOTE}</Note>}
        </div>

        {/* 繋いでいるときだけ。押せないログアウトを描いても意味が無い */}
        {online && (
          <Section title="接続">
            <Note>
              ログアウトすると保存済みの認証情報も消えます。次の起動では自動ログインしません。
            </Note>
            <div><Button variant="danger" onClick={() => void ggsApi.disconnect()}>ログアウト</Button></div>
          </Section>
        )}
      </div>
    </div>
  );
}

/* 説明文。**折り返しは `--w-text` (720px) で止める (規則 73)。**
 * 全幅の画面で 1 行が 1500px になると、目が行頭に戻れない。
 * 表・一覧・盤は窓いっぱいでよいので、折り返すのは文章だけ。 */
/** 条件式 1 つ。木のまま組ませ、保存するときだけ文字列に戻す */
function FormulaField({ label, src, onSave }: { label: string; src: string; onSave: (s: string) => void }) {
  const [cond, setCond] = useState<Cond | null>(() => (src ? parseCond(src) : null));
  /* **逃げ道を必ず残す (規則 30)。** 木で表せない式が実際にあり、
   * その場合に画面から手が出せなくなる。`onRaw` を渡さないと部品が
   * 「式を直接書く」を出さないので、ここで渡すのを忘れない。 */
  const [raw, setRaw] = useState<string | null>(null);
  if (raw !== null) {
    return (
      <Field label={label}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
          <TextField mono value={raw} onChange={setRaw}
                     placeholder="例: rated & or>=1600 & !synchro" />
          <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
            <Button onClick={() => { setRaw(null); setCond(raw ? parseCond(raw) : null); }}>
              木に戻す
            </Button>
            <span style={{ flex: 1 }} />
            <Button variant="primary" onClick={() => { onSave(raw); setRaw(null); setCond(raw ? parseCond(raw) : null); }}>
              反映する
            </Button>
          </div>
        </div>
      </Field>
    );
  }
  return (
    <Field label={label}>
      <FormulaEditor value={cond} onChange={setCond} onClear={() => setCond(null)}
                     onSave={onSave} onRaw={(s) => setRaw(s)} />
    </Field>
  );
}

/* ---------------- 対局結果 ----------------
 *
 * 終わった対局はバックエンドが 200 局ぶん控えていて、起動し直しても残る。
 * これまで見る場所が無く、**溜まるだけで一度も読めなかった**。
 *
 * レートの推移を上に、対局の一覧を下に置く。行を押すと検討で開く —
 * 「負けた対局を後から読む」がこの画面の用事なので、そこへ行けないと
 * 一覧を見せる意味が薄い。
 */
function GgsResults({ snap, onKifu }: {
  snap: GgsSnapshot;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  /* **既定は「すべて」にしない。** レートは形式ごとに別のプールで、
     通しの値は存在しない。混ぜて折れ線にすると、8 と 8r を行き来する
     たびに 150 点跳ぶ嘘のグラフになる。いちばん指している形式を出す。 */
  const [gtype, setGtype] = useState('');
  /* サーバーの履歴も取り込む。**手元に残るのはこのアプリで終局した対局
   * だけ**なので、別の端末や以前の GUI で打ったぶんが丸ごと抜けていた
   * (この画面が空のままになる)。GGS は `/os history` で全部返す。 */
  const login = snap.login;
  // 要求は空文字 (= 自分)、**保存されるキーは login** — バックエンドが
  // 他人の履歴と同じ map に入れるので、自分だけ空キーにはならない
  useEffect(() => { if (login) void ggsApi.history('').catch(() => {}); }, [login]);
  // 形式ごとにレートのプールが違うので、混ぜて折れ線にすると嘘になる
  const kinds = [...new Set([
    ...snap.results.map((r) => baseType(r.base, r.raw)),
    ...(snap.history[snap.login] ?? []).map((h) => baseType(h.gtype)),
  ])].filter(Boolean);
  /* 手元の記録に無い対局をサーバーの履歴から補う。**突き合わせるのは
     書庫の番号。** 手元の `id` は対局の番号 (`.64`) で、サーバーの履歴が
     返すのは書庫の番号 (`.84058`) — 別物なので、いつも一致せず**全局が
     二重に並んでいた**。同じ対局が 2 行あると、形式や勝敗の数え方まで
     ずれる (通常の一覧にランダム開局の対局が混ざって見えた)。 */
  const known = new Set(snap.results.flatMap((r) => [r.archive, r.id].filter(Boolean)));
  const fromServer: GameResult[] = (snap.history[snap.login] ?? [])
    .filter((h) => !known.has(h.id))
    .map((h) => {
      const iAmBlack = h.black === snap.login;
      const diff = parseFloat(h.score);
      return {
        id: h.id, seq: 0, base: h.gtype, opp: iAmBlack ? h.white : h.black,
        my_diff: Number.isFinite(diff) ? (iAmBlack ? diff : -diff) : null,
        my_rating: parseFloat(iAmBlack ? h.black_rating : h.white_rating) || null,
        at: Date.parse(h.at) / 1000 || 0,
        // 棋譜は手元に無いが、番号で取り出せる (規則 71 の覆いが引き受ける)
        kifu: '', ggf: '', archive: h.id,
      } as GameResult;
    });
  /* **時刻で並べ直す。** 手元の記録とサーバーの履歴を繋いだだけだと、
     それぞれの中では新しい順でも、境目で時間が巻き戻る (グラフの横軸が
     8/16 → 8/15 と逆走していた)。時刻の無いものは後ろへ。 */
  const all = [...snap.results, ...fromServer].sort((x, y) => (y.at ?? 0) - (x.at ?? 0));
  /* 既定 (未選択) は局数のいちばん多い形式。**「すべて」を既定にしない** —
     レートは形式ごとに別のプールなので、通しの推移は存在しない。 */
  let most = '';
  let mostN = -1;
  for (const k of kinds) {
    const n = all.filter((r) => baseType(r.base, r.raw) === k).length;
    if (n > mostN) { mostN = n; most = k; }
  }
  const cur = gtype || most || 'all';
  const rows = all.filter((r) => cur === 'all' || baseType(r.base, r.raw) === cur);
  // グラフは古い順。results は新しい順に積まれている
  const rated = rows.filter((r) => r.my_rating != null).reverse();
  const rates = rated.map((r) => r.my_rating as number);
  // 横軸のラベル。端と中央だけ出るので全部渡してよい
  const rateDates = rated.map((r) => fmtDay(r.at ?? 0));
  /* かざした点の一言。**どの対局のレートかが分からない**という声があった。
     日時・相手・石差まで出せば、下の表のどの行かが読める。 */
  const rateLabels = rated.map((r) => {
    const d = r.my_diff;
    const sign = d == null ? '' : d > 0 ? `+${d}` : `${d}`;
    return `${fmtDay(r.at ?? 0)} · ${r.opp} · ${sign} · ${Math.round(r.my_rating as number)}`;
  });
  /* グラフと表の対応付け。**同じ対局を指す**ので、点の番号ではなく
     対局そのもので突き合わせる (表は新しい順、グラフは古い順)。 */
  const [hover, setHover] = useState<number | null>(null);
  const hoverKey = hover != null && rated[hover] ? rowKey(rated[hover]) : '';

  return (
    /* **縦のスクロールは一覧だけ。** 画面ごと流れると、行が増えたときに
       グラフが上へ逃げて、対応付けを見ながら表を辿れない */
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column',
                  padding: 'var(--sp-4) var(--sp-4) 0' }}>
      <Section title="レートの推移"
               aside={kinds.length > 1 ? (
                 <Segmented value={cur} onChange={setGtype}
                            options={[...kinds.map((k) => ({ value: k, label: gtypeLabel(k) })),
                                      { value: 'all', label: 'すべて' }]} />
               ) : undefined}>
        {/* 本文の幅いっぱいに置くので viewBox もそれに合わせる
            (300 のままだと横に引き伸ばされて線の太さが崩れる)。
            **ここは画面の主役なので軸を出す** — 「いつ何点だったか」を
            読む場所 (2026-08-10 にデザイン側と決着。ドックの中は軸なし) */}
        {cur === 'all' ? (
          /* **通しのレートは無い。** 形式ごとに別のプールなので、
             ここで折れ線を引くと 8 と 8r の行き来がそのまま段差になる。
             一覧のほうは「すべて」で見たいことがあるので残す。 */
          <Note>レートは形式ごとに別です。推移は形式を選ぶと出ます。</Note>
        ) : (
          <RateChart points={rates} width={800} height={180} axes dates={rateDates}
                     labels={rateLabels} hover={hover} onHover={setHover} />
        )}
      </Section>

      <Section title="終わった対局" aside={<span>{rows.length}</span>} grow>
        {!rows.length && <Empty>まだ記録がありません。</Empty>}
        {/* 行どうしは詰める (節の余白が行間に入る)。
            **形式を変えたら作り直す。** 同じ器に別の一覧を流し込むと、
            前の行が残ることがあった (件数 11 に対し 14 行が描かれ、通常の
            一覧にランダム開局の対局が居座って見えた)。 */}
        <List key={cur}>
        {rows.map((r) => (
          <ResultRow key={rowKey(r)} opponent={r.opp}
                     // グラフでかざしている対局を光らせる
                     picked={!!hoverKey && rowKey(r) === hoverKey}
                     onHover={(on) => {
                       const i = rated.findIndex((x) => rowKey(x) === rowKey(r));
                       setHover(on ? (i < 0 ? null : i) : null);
                     }}
                     win={(r.my_diff ?? 0) > 0} draw={r.my_diff === 0}
                     discs={r.my_diff ?? 0} when={fmtDay(r.at)}
                     note={cur === 'all' ? gtypeLabel(baseType(r.base, r.raw)) : undefined}
                     rating={r.my_rating}
                     // GGF なら開始局面も入っている (抽選開局の対局が戻る)。
                     // どちらも無い対局は番号から取り出す
                     onClick={() => onKifu(`${r.opp} との対局`, r.ggf || r.kifu, r.archive)}
                     dim={!r.ggf && !r.kifu && !r.archive} />
        ))}
        </List>
      </Section>
    </div>
  );
}

/** 一覧の行の鍵。
 *
 * **繋ぐだけでは衝突する。** `id + seq` は `.6` + `86` と `.68` + `6` が
 * どちらも `.686` になる。鍵が重なると、絞り込みを切り替えたときに前の
 * 行が DOM に残り、**件数と表示行数が合わなくなる** (通常の一覧に
 * ランダム開局の対局が居座って見えた)。 */
const rowKey = (r: GameResult) => `${r.id}#${r.seq}`;

/** 対局の種別。
 *
 * 履歴から来た結果は `base` が `"s8r16.2024..."` の形なので頭を取れば済む。
 * **終局の報せから作った結果は `base` が `.11` のような対局 id** で、
 * 切っても空にしかならない (画面に `?` が出ていた)。そちらは生の行に
 * 形式が入っているので拾う。 */
const baseType = (base: string, raw?: string) => {
  /* **生の行にある形式を先に見る。** `base` が対局の番号 (`.64`) のときは
     頭が空になるので raw へ落ちるが、番号でない `base` が来ることもある。
     どちらでも生の行の形式が正しいので、そちらを優先する。 */
  const fromRaw = raw?.split(/\s+/).find((t) => /^s?8(r\d+)?$/.test(t));
  if (fromRaw) return fromRaw;
  return base.split('.')[0] ?? '';
};

/** 履歴の日時 (`30 Jul 2026 17:36:36`) を `7/30 17:36` にする。
 *
 * **読めなければ元の文字をそのまま返す。** サーバーの書式が変わったときに
 * 空欄になるより、生のまま出ているほうが直しやすい。 */
function fmtWhen(at: string): string {
  const t = Date.parse(at);
  if (!Number.isFinite(t)) return at;
  const d = new Date(t);
  const p2 = (n: number) => String(n).padStart(2, '0');
  return `${d.getMonth() + 1}/${p2(d.getDate())} ${p2(d.getHours())}:${p2(d.getMinutes())}`;
}

/** 終わった日。今日なら時刻だけ。 */
function fmtDay(secs: number): string {
  if (!secs) return '';
  const d = new Date(secs * 1000);
  const now = new Date();
  const p2 = (n: number) => String(n).padStart(2, '0');
  const same = d.toDateString() === now.toDateString();
  return same ? `${p2(d.getHours())}:${p2(d.getMinutes())}` : `${d.getMonth() + 1}/${p2(d.getDate())}`;
}


/* プレイヤーの名刺 (設計 §6 の「浮くもの」)。
 *
 * **全画面の詳細を置き換えるものではなく、その前の下読み。**「詳しく見る」で
 * 全画面へ渡す。対局中に相手を調べたいときに、盤から離れずに済むのが値打ち。
 *
 * 絵は 在室 / 最終対局 / 直近 10 戦 / レート推移 / 勝敗表 / 観戦 も描いて
 * いたが、**エンジンにもサーバーにも口が無い**ことを伝えたら絵から落ちた。
 * いま出せるのは finger の「対局の申し込み」4 行だけ。
 *
 * **対戦履歴は載せない。** 340px の名刺に 1 行 3 項目を詰めると溢れて
 * 読めず、表にする幅も無い。履歴は「詳しく見る」が表で持つ。
 */
function UserCard({ snap, name, onClose, onDetail, onAsk }: {
  snap: GgsSnapshot; name: string;
  onClose: () => void;
  onDetail: () => void;
  onAsk: () => void;
}) {
  useEffect(() => { ggsApi.finger(name).catch(() => {}); }, [name]);

  const u = snap.users.find((x) => x.name === name);
  const rates = bothRates(u);
  const fields = snap.fingers[name]?.fields ?? [];

  /* 「対局の申し込み」のうち**条件式でない 4 行**だけ。条件式は木で描くもので、
     340px の名刺には収まらない (全画面の詳細が持つ) */
  const facts = (fingerGroups(fields).find((g) => g.title === '対局の申し込み')?.rows ?? [])
    .filter((r) => !['accept', 'decline', 'request'].includes(normKey(r.key).replace(/\(.*\)/, '')));

  return (
    <Overlay onClose={onClose}>
      <Modal title={name} onClose={onClose}
             sub={rates || undefined}
             scroll
             actions={<>
               <Button size="field" onClick={onDetail}>詳しく見る</Button>
               <span style={{ marginLeft: 'auto' }} />
               <Button size="field" variant="primary" onClick={onAsk}>申し込む</Button>
             </>}>
        <Section title="対局の申し込み">
            {!facts.length && <Empty>読み込んでいます…</Empty>}
            {facts.map((r) => (
              <div key={r.key} style={{ display: 'flex', alignItems: 'center',
                                        gap: 'var(--sp-3)', fontSize: 'var(--fs-5)' }}>
                <span style={{ color: 'var(--sub)' }}>{r.label}</span>
                {/* **値も読み下す。** サーバーは `1` / `+` / `0` を返すので、
                    そのままだと「申し込み受付 1」になって意味が読めない。
                    全画面の詳細は前から `fingerValue` を通していたのに、
                    名刺だけ生のまま出していた */}
                <span style={{ marginLeft: 'auto', overflow: 'hidden',
                               textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {fingerValue(r.key, r.value) || '—'}
                </span>
              </div>
            ))}
        </Section>
      </Modal>
    </Overlay>
  );
}

/* ---------------- プレイヤー ----------------
 *
 * 接続中の一覧と、選んだ人の詳細。詳細の頭には名前だけでなくレートと
 * 対局の状況も載せる — 一覧で見えていたものが詳細で消えると、
 * 申し込む前に一覧へ戻る羽目になる。
 */
function GgsUsers({ snap, onNav, onKifu }: {
  snap: GgsSnapshot; onNav: (id: NavId) => void;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  /* 一覧 → 名刺 → 全画面の詳細、の 3 段 (設計 §6)。**一覧から直に全画面へ
     飛ばさない** — 相手を軽く見たいだけのときに一覧が消えるのは重い。
     `card` が名刺、`sel` が全画面。 */
  const [card, setCard] = useState<string | null>(null);
  const [mode, setMode] = useState<'who' | 'top'>('who');
  /* プレイヤー一覧の列。**見出しと行が同じ配列を見る。** `#` は順位なので
     「上位」のときだけ立ち、そのぶん先頭が 1 つ増える — 条件が 2 か所に
     あると片方だけ直して列がずれる。 */
  const userCols: Col[] = [
    ...(mode === 'top' ? [{ head: '#', w: 26, right: true, num: true } as Col] : []),
    { head: '名前', clip: true },
    // 接続中はプールを選んでいないので、通常 8 とランダム開局 8r を並べる
    { head: mode === 'who' ? '通常' : 'レート', w: 96, right: true, num: true },
    ...(mode === 'who' ? [{ head: 'ランダム', w: 104, right: true, num: true } as Col] : []),
    // 受付は「申し込んで通るか」。**対局中でも受けることがある**ので
    // 状態とは別の列にする (`open > 対局中の数` なら受ける)
    ...(mode === 'who' ? [{ head: '受付', w: 56, right: true } as Col] : []),
    { head: '状態', w: 52, right: true },
  ];
  /* 一覧の「状態」列は進行中の対局 (`snap.ongoing`) から出す。**その一覧は
     ログイン時と 60 秒ごとにしか届かない**ので、繋いだ直後にこの画面を
     開くと全員の状態が空になる。開いた時点で聞き直す — `tell /os match` は
     一覧を返すだけで何も動かさない (規則 61 — 直し方は操作のそばに、の
     裏返しで、聞けば済むものを人に待たせない)。 */
  useEffect(() => { void ggsApi.listMatches().catch(() => {}); }, []);
  /* 確認用の入口。**繋がずに名刺の枠だけ撮る**ための道
     (`KUROOBI_GGS_AUTOVIEW=users:card`)。中身は空のままでよく、
     見たいのは 340 の器・3 段・足の 2 つの釦 */
  useEffect(() => {
    void ggsApi.autoview().then((v) => {
      /* **前半は行き先の id と同じ綴りにする。** `ggs-users` から
         `ggs-players` に変えたときここだけ古いままで、`users:card` は
         当たらないうえに nav が無い行き先に落ちて本文が真っ白になった */
      if (v === 'players:card') setCard(snap.login || '—');
      if (v === 'players:top') setMode('top');
    }).catch(() => {});
  }, [snap.login]);
  const [sel, setSel] = useState<string | null>(null);
  // レートは形式ごとに別のプール。混ぜると順位が意味を持たないので、
  // ランキングは見たいプールを選べるようにする (自分のレートは両方出る)
  const [pool, setPool] = useState('8');
  // 順位を探す場所なので頁で切る (規則 74)。スクロールだと「いま何位を
  // 見ているか」が分からなくなる
  const [page, setPage] = useState(0);
  const [perPage, setPerPage] = useState(25);
  const [tab, setTab] = useState('プロフィール');

  if (sel) {
    return <UserDetail snap={snap} name={sel} tab={tab} onTab={setTab}
                       onBack={() => setSel(null)} onNav={onNav} onKifu={onKifu} />;
  }

  const rows = mode === 'who' ? snap.users : snap.ranking;
  const mine = snap.my_ranks;
  const pages = Math.max(1, Math.ceil(rows.length / perPage));
  const cur = Math.min(page, pages - 1);
  const slice = rows.slice(cur * perPage, (cur + 1) * perPage);

  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4) var(--sp-4) 0' }}>
      {card && (
        <UserCard snap={snap} name={card}
                  onClose={() => setCard(null)}
                  onDetail={() => { setSel(card); setCard(null); }}
                  onAsk={() => { setCard(null); onNav('ggs-lobby'); }} />
      )}
      <Section title="自分のレート"
               aside={<Button onClick={() => {
                 for (const t of ['8', '8r']) void ggsApi.rank(t, snap.login);
               }}>更新</Button>}>
        {!mine.length && <Empty>まだ記録がありません。</Empty>}
        {mine.map((r) => (
          <RateRow key={r.gtype} label={gtypeLabel(r.gtype)}
                   rate={{ value: r.rating, dev: r.dev, rank: r.rank,
                           w: r.wins, l: r.losses, d: r.draws,
                           provisional: r.dev > PROVISIONAL_DEV }} />
        ))}
      </Section>

      <Section title={mode === 'who' ? '接続中' : 'ランキング'}
               aside={<>
                 {/* プールは上位のときだけ選ばせる。接続中の一覧は
                     プールに関係なく同じ顔ぶれなので、出すと嘘になる */}
                 {mode === 'top' && (
                   <Segmented value={pool} onChange={(p) => {
                     setPool(p);
                     void ggsApi.top(p, 100);
                   }} options={[{ value: '8', label: '通常' }, { value: '8r', label: 'ランダム開局' }]} />
                 )}
                 <Segmented value={mode} onChange={(m) => {
                   setMode(m);
                   setPage(0);
                   if (m === 'who') void ggsApi.who(); else void ggsApi.top(pool, 100);
                 }} options={[{ value: 'who', label: '接続中' }, { value: 'top', label: '上位' }]} />
               </>}>
        {!rows.length && <Empty>いません。</Empty>}
        {/* 設計 §5 はこの一覧を**表**として描いている — 列見出し (# / 名前 /
            レート / 状態) と 24px の行。規則 5 の「`--h-row` 24px = 表の行」に
            合う。以前は 32px の行で列見出しも無く、レートと状態の位置が
            行ごとに動いていた。
            `#` は順位なので「上位」のときだけ出す (接続中の一覧に順位は無い) */}
        {!!rows.length && (
          <TableHead cols={userCols} pad="var(--sp-4)" />
        )}
        {/* 列見出しだけを残さない。繋いだ直後は一覧がまだ届いていない */}
        {!slice.length && (
          <Empty>{mode === 'who' ? '接続中の人はいません。' : '順位をまだ受け取っていません。'}</Empty>
        )}
        {/* 行どうしは詰める (節の余白が行間に入る) */}
        <List>
        {slice.map((u, i) => {
        const playing = snap.ongoing.some((o) => o.names.includes(u.name));
        return (
          <TableRow key={u.name} cols={userCols} pad="var(--sp-4)" onClick={() => setCard(u.name)}>
            {mode === 'top' && (
              <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>
                {cur * perPage + i + 1}
              </span>
            )}
            <span className="k-sel">{u.name}</span>
            {/* レートには必ず偏差を添える (規則 29) — 偏差が大きいと数字が
                意味を持たない。ランキング (`/os t`) は偏差を返すが、接続中の
                一覧 (`/os who`) は返さないので、そちらは数字だけになる。

                **接続中はプールを選んでいないので両方出す。** ランキングは
                プールを選んで見るものなので通常 8 の列だけを使う */}
            <span>
              {u.rating != null && <>
                {u.rating.toFixed(1)}
                {u.dev != null && (
                  <span style={{ color: 'var(--sub)', marginLeft: 4 }}>±{Math.round(u.dev)}</span>
                )}
              </>}
            </span>
            {mode === 'who' && (
              <span>
                {u.rating_r != null && <>
                  {u.rating_r.toFixed(1)}
                  {u.dev_r != null && (
                    <span style={{ color: 'var(--sub)', marginLeft: 4 }}>±{Math.round(u.dev_r)}</span>
                  )}
                </>}
              </span>
            )}
            {/* **受付状態。** サーバーは who の名前の次に印を出す
                (`+` 受ける / `-` 受けない / `x` 幽霊)。待機中でも受けない
                人が居り、申し込む前に分からないと空振りする。 */}
            {mode === 'who' && (
              <span style={{
                fontSize: 'var(--fs-6)',
                color: u.open === '+' ? 'var(--accent)'
                  : u.open === 'x' ? 'var(--bad)' : 'var(--sub)',
              }}>
                {u.open === '+' ? '受付中' : u.open === 'x' ? '切断' : u.open ? '受けない' : '—'}
              </span>
            )}
            {/* 状態は色つきの文字 (絵と同じ)。バッジにすると行の高さが動く。
                **対局していない人は「待機」と出す** — 空欄にすると、列ごと
                効いていないのか誰も対局していないのかが読み手に分からない
                (設計 §5 も 対局中 / 待機 の 2 値で描いている) */}
            <span style={{
              fontSize: 'var(--fs-6)', color: playing ? 'var(--ok)' : 'var(--sub)',
            }}>{playing ? '対局中' : '待機'}</span>
          </TableRow>
        );})}
        </List>
        {/* ページ送りは一覧の外。詰めた一覧のすぐ下だと行に見える */}
        {rows.length > perPage && (
          <div style={{
            display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
            padding: 'var(--sp-2) 0', fontSize: 'var(--fs-6)', color: 'var(--sub)',
          }}>
            <Button disabled={cur === 0} onClick={() => setPage(cur - 1)}>前へ</Button>
            <span style={{ fontVariantNumeric: 'tabular-nums' }}>{cur + 1} / {pages}</span>
            <Button disabled={cur >= pages - 1} onClick={() => setPage(cur + 1)}>次へ</Button>
            <span style={{ marginLeft: 'auto' }}>表示件数</span>
            <Select size="ctrl" value={String(perPage)}
                    options={[['25', '25'], ['50', '50'], ['100', '100']]}
                    onChange={(v) => { setPerPage(+v); setPage(0); }} />
          </div>
        )}
      </Section>
    </div>
  );
}

function UserDetail({ snap, name, tab, onTab, onBack, onNav, onKifu }: {
  snap: GgsSnapshot; name: string; tab: string;
  onTab: (t: string) => void; onBack: () => void; onNav: (id: NavId) => void;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  useEffect(() => { ggsApi.finger(name).catch(() => {}); }, [name]);
  useEffect(() => { ggsApi.history(name === snap.login ? '' : name).catch(() => {}); }, [name, snap.login]);

  const u = snap.users.find((x) => x.name === name);
  const rates = bothRates(u);
  const playing = snap.ongoing.some((o) => o.names.includes(name));
  const fields = snap.fingers[name]?.fields ?? [];
  // 自分の履歴も他人と同じく login のキーで入る (要求だけが空文字)
  const rows = snap.history[name] ?? [];
  const histCols: Col[] = [
    { head: '日時', w: 132 },
    { head: '形式', w: 104 },
    { head: '相手', clip: true },
    { head: '手番', w: 44 },
    { head: '石差', w: 64, right: true, num: true },
  ];

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      {/* 頭は帯。名前・レート・対局の状況・申し込みを 1 行に収める */}
      <div style={{
        flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        padding: 'var(--sp-3) var(--sp-4)', borderBottom: '1px solid var(--border)',
        background: 'var(--panel)',
      }}>
        <IconButton name="back" label="一覧へ戻る" onClick={onBack} />
        <span className="k-sel" style={{ fontSize: 'var(--fs-2)', fontWeight: 600 }}>{name}</span>
        {rates && (
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{rates}</span>
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
            {/* 均一に 24 行並べると、申し込む前に見たいものが埋もれる。
                まとまりと並び順は ggs.ts の FINGER_GROUPS が決める */}
            {fingerGroups(fields).map((g) => (
              <Section key={g.title} title={g.title}>
                {g.rows.map((r) => {
                  const key = normKey(r.key).replace(/\(.*\)/, '');
                  // 条件式は文字に潰さず木のまま描く。構造がそのまま意味になっている
                  const cond = ['accept', 'decline', 'request'].includes(key)
                    ? parseCond(r.value) : null;
                  return (
                    <div key={r.key} style={{
                      display: 'flex', gap: 'var(--sp-3)', alignItems: 'flex-start',
                      padding: 'var(--sp-2) 0', borderBottom: '1px solid var(--border-weak)',
                    }}>
                      <span style={{
                        width: 'var(--w-label)', flex: 'none', fontSize: 'var(--fs-6)', color: 'var(--sub)',
                      }}>{r.label}</span>
                      <span style={{ flex: 1, minWidth: 0, fontSize: 'var(--fs-5)' }}>
                        {cond ? <FormulaView node={cond} top />
                          : ['accept', 'decline', 'request'].includes(key) ? '指定なし'
                          : fingerValue(r.key, r.value)}
                      </span>
                    </div>
                  );
                })}
              </Section>
            ))}
          </>
        ) : (
          <>
            {!rows.length && <Empty>対戦履歴はありません。</Empty>}
            {/* **表で出す。** 1 行 3 項目を「·」で繋いだ 2 段だと、日時・
                形式・石差が行ごとに違う位置に来て縦に読めない。押すと
                棋譜を覆いで見せる (手元に棋譜は無いので番号から取り出す) */}
            {!!rows.length && <TableHead cols={histCols} />}
            <List>
            {rows.map((h) => {
              const black = h.black === name;
              const d = parseFloat(h.score);
              // 石差はこの人から見た値にする。黒番の記録は黒視点で来る
              const mine = Number.isFinite(d) ? (black ? d : -d) : null;
              return (
                <TableRow key={h.id} cols={histCols}
                          onClick={() => onKifu(`${h.black} 対 ${h.white}`, '', h.id)}>
                  <span style={{ color: 'var(--sub)' }}>{fmtWhen(h.at)}</span>
                  <span style={{ color: 'var(--sub)' }}>{gtypeLabel(h.gtype)}</span>
                  <span className="k-sel">{black ? h.white : h.black}</span>
                  <span style={{ color: 'var(--sub)' }}>{black ? '黒' : '白'}</span>
                  <span style={{
                    color: mine == null ? 'var(--sub)'
                      : mine > 0 ? 'var(--ok)' : mine < 0 ? 'var(--bad)' : 'var(--text)',
                  }}>
                    {mine == null ? '—' : mine > 0 ? `+${mine}` : `${mine}`}
                  </span>
                </TableRow>
              );
            })}
            </List>
          </>
        )}
      </div>
    </div>
  );
}

/** チャットの左 — 会話の一覧。**`--w-lobby` (174px)。手合い一覧の
 *  `--w-list` を借りない** (規則 6 — 役割が違う列は幅を借りない)。
 *
 *  `GgsChat` が 187 行あり、左の一覧・中央の会話・下の入力に割れて
 *  いたので左を出した。props は 4 つで済む。 */
function ChatList({ sorted, cur, onThread, onPick }: {
  sorted: [string, { last: ChatMsg; n: number }][];
  cur: string;
  onThread: (t: string) => void;
  onPick: (v: boolean) => void;
}) {
  return (
    <>
    {/* 会話の一覧は `--w-lobby` (174px)。**手合い一覧の `--w-list` を
        借りない** — 規則 6 は「役割が違う列は幅を借りない」と決めている。
        設計 §6 のこの列も 173px で、`--w-lobby` の「GGS 左カラム」という
        説明はここのことだった (ロビーには 174 幅の列が無い)。
        相手の名前は溢れたら省略記号で切る。 */}
    <aside style={{
      width: 'var(--w-lobby)', flex: 'none', borderRight: '1px solid var(--border)',
      minHeight: 0, display: 'flex', flexDirection: 'column',
    }}>
      {/* 見出しの釦は `chip` (20px)。**列が 174px しかないので、28px の
          釦だと「会話」と押し合いになる**。絵の帯は 21px だが、それだと
          釦が罫にめり込む (Section の見出しで一度指摘が出ている) ので
          帯だけ 32px にした。要 push */}
      <div style={{
        flex: 'none', height: 'var(--h-field)', display: 'flex', alignItems: 'center',
        gap: 'var(--sp-2)', padding: '0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
      }}>
        <span style={{ fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)' }}>会話</span>
        <span style={{ marginLeft: 'auto' }} />
        <Button size="chip" onClick={() => onPick(true)}>新しい相手</Button>
      </div>
      <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>
      {sorted.map(([key, info]) => (
        <button key={key} type="button" onClick={() => onThread(key)}
          aria-current={key === cur || undefined}
          className={'k-row' + (key === cur ? ' k-on' : '')}
          style={{
            width: '100%', border: 0, textAlign: 'left', display: 'flex', flexDirection: 'column',
            gap: 'var(--sp-1)', padding: 'var(--sp-2) var(--sp-3)',
            borderBottom: '1px solid var(--border-weak)',
            ...picked(key === cur),
          }}>
          <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-5)' }}>
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
    </>
  );
}
