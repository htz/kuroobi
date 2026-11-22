import { useCallback, useEffect, useRef, useState } from 'react';
import { api, emitApp, ggsApi, onApp, type ControlInfo, type SavedServer } from './api';
import type { GgsSnapshot } from './types';
import type { Prefs } from './prefs';
import type { Account } from './components/accounts';
import { DEFAULT_DECLINE, fingerKey } from './ggs';
import { t, tErr, useLang } from './i18n';
import { Confirm, Field, ListRow, marked, Modal, Overlay } from './components/layout';
import { ListHead } from './ggsui';
import { Button, Dot, Segmented, Select, TextField } from './components/primitives';
import { Tag } from './components/ggs';
import { EngineSettings, SettingGroup, SettingRow, SettingToggle, ViewSettings, type LearnSetting } from './Dialogs';
import { FormulaField } from './GgsScreens';
import { openNewLogin } from './ggs/Login';
import { markApplied } from './applied';
import { whenFull } from './when';

export const SETTINGS_CATS = ['engine', 'view', 'ai', 'ggs-me', 'ggs-account', 'ggs-play', 'ggs-form'] as const;
export type SettingsCat = typeof SETTINGS_CATS[number];

const groups = (): { title: string; items: [SettingsCat, string][] }[] => [
  { title: t('settings.group.app'), items: [['engine', t('settings.tab.engine')], ['view', t('settings.tab.view')], ['ai', t('settings.tab.ai')]] },
  {
    title: 'GGS', items: [
      ['ggs-me', t('settings.cat.me')],
      ['ggs-account', t('settings.cat.account')],
      ['ggs-play', t('settings.cat.play')],
      ['ggs-form', t('settings.cat.form')],
    ],
  },
];

export function settingsTitle(cat: SettingsCat): string {
  const label = groups().flatMap((g) => g.items).find(([id]) => id === cat)?.[1] ?? '';
  return cat.startsWith('ggs-') ? `GGS · ${label}` : label;
}

/** The settings screen: categories on the left, rows on the right. Every change applies at once. */
export function SettingsScreen({ cat, onCat, prefs, setPref, snap, onLogin, learn }: {
  cat: SettingsCat; onCat: (c: SettingsCat) => void;
  /** Leaves settings for the login screen (to add an account). */
  onLogin: () => void;
  learn: LearnSetting;
  prefs: Prefs; setPref: <K extends keyof Prefs>(k: K, v: Prefs[K]) => void;
  snap: GgsSnapshot | null;
}) {
  useLang();
  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <div className="k-scroll k-listpane" style={{
        flex: 'none', borderRight: '1px solid var(--border)',
        display: 'flex', flexDirection: 'column', paddingBottom: 'var(--sp-2)',
      }}>
        {groups().map((g) => (
          <div key={g.title} style={{ display: 'flex', flexDirection: 'column' }}>
            <ListHead>{g.title}</ListHead>
            {g.items.map(([id, label]) => {
              const on = id === cat;
              return (
                <ListRow key={id} on={on} rule={false} onClick={() => onCat(id)} style={{ fontWeight: on ? 600 : 400 }}>
                  {label}
                </ListRow>
              );
            })}
          </div>
        ))}
      </div>
      <div className="k-scroll" style={{ flex: 1, minWidth: 0, minHeight: 0, padding: 'var(--sp-4) var(--sp-1) 0' }}>
        <div style={{ maxWidth: 'var(--w-settings)', display: 'flex', flexDirection: 'column' }}>
          {cat === 'engine' && <EngineSettings learn={learn} />}
          {cat === 'view' && <ViewSettings prefs={prefs} setPref={setPref} />}
          {cat === 'ai' && <AiSettings />}
          {cat === 'ggs-me' && (snap && snap.conn === 'online' ? <GgsMeSettings snap={snap} /> : <Offline />)}
          {cat === 'ggs-account' && <GgsAccountSettings snap={snap} onLogin={onLogin} />}
          {cat === 'ggs-play' && (snap && snap.conn === 'online'
            ? <GgsPlaySettings snap={snap} learn={prefs.learnGgs} setLearn={(on) => setPref('learnGgs', on)} /> : <Offline />)}
          {cat === 'ggs-form' && (snap && snap.conn === 'online'
            ? <GgsFormSettings snap={snap} /> : <Offline />)}
        </div>
      </div>
    </div>
  );
}

const MCP_CLIENTS = ['claude', 'desktop', 'cursor', 'codex', 'gemini', 'vscode'] as const;
type McpClient = typeof MCP_CLIENTS[number];

/** Tab label, numbered steps, and the step that holds the snippet, for each AI app. */
function mcpClient(c: McpClient): { label: string; steps: string; at: number } {
  switch (c) {
    case 'claude': return { label: 'Claude Code', steps: t('settings.ai.steps_claude'), at: 0 };
    case 'desktop': return { label: 'Claude Desktop', steps: t('settings.ai.steps_desktop'), at: 1 };
    case 'cursor': return { label: 'Cursor', steps: t('settings.ai.steps_cursor'), at: 0 };
    case 'codex': return { label: 'Codex CLI', steps: t('settings.ai.steps_codex'), at: 0 };
    case 'gemini': return { label: 'Gemini CLI', steps: t('settings.ai.steps_gemini'), at: 0 };
    case 'vscode': return { label: 'VS Code', steps: t('settings.ai.steps_vscode'), at: 0 };
  }
}

/** The same server (`kuroobi-mcp --mcp`) in each app's registration format. */
function mcpSnippet(c: McpClient, exe: string): string {
  const entry = { command: exe, args: ['--mcp'] };
  switch (c) {
    case 'claude': return `claude mcp add kuroobi-gui -- ${JSON.stringify(exe)} --mcp`;
    case 'codex': return `[mcp_servers.kuroobi-gui]\ncommand = ${JSON.stringify(exe)}\nargs = ["--mcp"]`;
    case 'vscode': return JSON.stringify({ servers: { 'kuroobi-gui': { type: 'stdio', ...entry } } }, null, 2);
    default: return JSON.stringify({ mcpServers: { 'kuroobi-gui': entry } }, null, 2);
  }
}

function StepNumber({ n }: { n: number }) {
  return (
    <span style={{
      flex: 'none', width: 20, height: 20, borderRadius: 'var(--r-pill)', background: 'var(--card)',
      border: '1px solid var(--border)', display: 'grid', placeItems: 'center',
      fontSize: 'var(--fs-6)', fontWeight: 600, fontVariantNumeric: 'tabular-nums',
    }}>{n}</span>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(id);
  }, [copied]);
  return (
    <Button size="row" onClick={() => void navigator.clipboard.writeText(text).then(() => setCopied(true))}>
      {copied ? t('settings.ai.copied') : t('settings.ai.copy')}
    </Button>
  );
}

/** Whether AI tools may drive the app, and how to register the MCP server. */
function AiSettings() {
  const [info, setInfo] = useState<ControlInfo | null>(null);
  const [err, setErr] = useState('');
  const load = useCallback(() => { void api.controlInfo().then(setInfo).catch((x) => setErr(tErr(x))); }, []);
  useEffect(load, [load]);
  const [client, setClient] = useState<McpClient>('claude');
  if (!info) return null;
  const app = mcpClient(client);
  const snippet = mcpSnippet(client, info.ctl);
  const [tone, state]: ['ok' | 'sub' | 'gold', string] = info.forced_off ? ['gold', t('settings.ai.forced_off')]
    : info.active ? ['ok', t('settings.ai.listening')]
    : info.enabled ? ['gold', t('settings.ai.taken')] : ['sub', t('settings.ai.closed')];
  return (
    <>
      <SettingGroup title={t('settings.ai.control')}>
        <SettingRow label={t('settings.ai.accept')} help={t('settings.ai.help')} error={err || undefined}>
          <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-ctrl)' }}>
            <SettingToggle checked={info.enabled} onChange={(on) => {
              void api.setControl(on).then(() => { setErr(''); markApplied(); load(); })
                .catch((x) => setErr(tErr(x)));
            }} />
            <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)', fontSize: 'var(--fs-5)' }}>
              <Dot tone={tone} />{state}
            </span>
          </span>
          {info.enabled && !info.forced_off && (
            <span className="k-sel" style={{
              fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', color: 'var(--sub)', wordBreak: 'break-all',
            }}>{info.socket}</span>
          )}
        </SettingRow>
      </SettingGroup>
      <SettingGroup title={t('settings.ai.mcp')} aside={t('settings.ai.mcp_aside')}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)', paddingTop: 'var(--sp-3)' }}>
          <div style={{ maxWidth: '100%', overflowX: 'auto' }}>
            <Segmented value={client} onChange={setClient}
                       options={MCP_CLIENTS.map((c) => ({ value: c, label: mcpClient(c).label }))} />
          </div>
          <ol style={{ margin: 0, padding: 0, listStyle: 'none', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
            {app.steps.split('\n').filter(Boolean).map((line, i) => (
              <li key={`${client}-${i}`} style={{ display: 'flex', gap: 'var(--sp-3)', alignItems: 'flex-start' }}>
                <StepNumber n={i + 1} />
                <div style={{
                  flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
                  fontSize: 'var(--fs-5)', lineHeight: '20px',
                }}>
                  <span>{line}</span>
                  {i === app.at && <>
                    <pre className="k-sel" style={{
                      margin: 0, padding: 'var(--sp-2h) var(--sp-3)', overflowX: 'auto', background: 'var(--bg)',
                      border: '1px solid var(--border)', borderRadius: 'var(--r-3)', fontFamily: 'var(--ff-mono)',
                      fontSize: 'var(--fs-6)', lineHeight: 1.6, whiteSpace: 'pre', userSelect: 'text',
                    }}>{snippet}</pre>
                    <span style={{ display: 'flex', justifyContent: 'flex-end' }}>
                      <CopyButton key={client} text={snippet} />
                    </span>
                  </>}
                </div>
              </li>
            ))}
          </ol>
        </div>
      </SettingGroup>
    </>
  );
}

function Offline() {
  return (
    <div style={{ padding: 'var(--sp-5) var(--sp-3)', fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>
      {t('settings.ggs.offline')}
    </div>
  );
}

/** The saved servers and accounts, re-read whenever another screen changes them. */
export function useServers() {
  const [servers, setServers] = useState<SavedServer[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [auto, setAuto] = useState<[string, string] | null>(null);
  const reload = useCallback(() => {
    void ggsApi.servers().then(setServers).catch(() => {});
    void ggsApi.accounts().then(setAccounts).catch(() => {});
    void ggsApi.autoAccount().then(setAuto).catch(() => {});
  }, []);
  useEffect(() => {
    reload();
    const off = onApp('servers-changed', reload);
    return () => { void off.then((f) => f()); };
  }, [reload]);
  return { servers, accounts, auto };
}

export const serverName = (servers: SavedServer[], addr: string): string =>
  servers.find((s) => s.addr === addr)?.name ?? addr;

const changed = () => emitApp('servers-changed');

/** The logged-in account: who and where, log out, profile text and password. */
function GgsMeSettings({ snap }: { snap: GgsSnapshot }) {
  const { servers } = useServers();
  return (
    <SettingGroup title={t('settings.account.account')}>
      <SettingRow label={t('settings.account.logged_in')} help={t('settings.account.logout_help')}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-ctrl)' }}>
          <b className="k-sel" style={{ fontWeight: 600 }}>{snap.login}</b>
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{serverName(servers, snap.server)}</span>
          <span className="k-sel" style={{ fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{snap.server}</span>
        </span>
        <Button variant="danger" onClick={() => void ggsApi.disconnect().then(changed)}>{t('ggs.settings.logout')}</Button>
      </SettingRow>
      <ProfileRows snap={snap} />
    </SettingGroup>
  );
}

function GgsAccountSettings({ snap, onLogin }: { snap: GgsSnapshot | null; onLogin: () => void }) {
  const { servers, accounts, auto } = useServers();
  const [edit, setEdit] = useState<SavedServer | 'new' | null>(null);
  const [drop, setDrop] = useState<{ kind: 'server'; addr: string } | { kind: 'account'; addr: string; login: string } | null>(null);
  const online = snap?.conn === 'online';
  const cur = online ? `${snap!.server}|${snap!.login}` : '';
  const autoKey = auto ? `${auto[0]}|${auto[1]}` : '';
  const autoOpts: [string, string][] = [['', t('settings.account.auto_none')],
    ...accounts.map((a) => [`${a.server}|${a.login}`, `${a.login} · ${serverName(servers, a.server)}`] as [string, string])];

  return (
    <>
      <SettingGroup title={t('settings.account.login')}>
        <SettingRow label={t('settings.account.auto')} help={t('settings.account.auto_help')}>
          <Select value={autoKey} options={autoOpts}
                  onChange={(v) => {
                    const [server, login] = v ? v.split('|') : ['', ''];
                    void ggsApi.setAutoAccount(server, login).then(() => { changed(); markApplied(); });
                  }} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t('settings.account.servers')}
                    aside={t('settings.account.count', { s: servers.length, a: accounts.length })}>
        <div style={{ display: 'flex', flexDirection: 'column' }}>
          {servers.map((sv) => {
            const mine = accounts.filter((a) => a.server === sv.addr);
            return (
              <div key={sv.addr} style={{ display: 'flex', flexDirection: 'column' }}>
                <div style={{
                  display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-row2)',
                  padding: '0 var(--sp-3)', marginTop: 'var(--sp-2)', borderRadius: 'var(--r-1)',
                  background: 'var(--card)', whiteSpace: 'nowrap',
                }}>
                  <b style={{ fontWeight: 600 }}>{sv.name}</b>
                  <span className="k-sel" style={{ fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', color: 'var(--sub)',
                                                   overflow: 'hidden', textOverflow: 'ellipsis' }}>{sv.addr}</span>
                  <Tag tone={sv.public ? 'accent' : 'sub'}>{sv.public ? t('settings.server.public') : t('settings.server.test')}</Tag>
                  <span style={{ marginLeft: 'auto', display: 'flex', gap: 'var(--sp-1)' }}>
                    <Button size="row" variant="ghost" onClick={() => setEdit(sv)}>{t('settings.server.edit')}</Button>
                    <Button size="row" variant="ghost" onClick={() => setDrop({ kind: 'server', addr: sv.addr })}>{t('ggs.login.remove')}</Button>
                  </span>
                </div>
                {mine.map((a) => {
                  const key = `${a.server}|${a.login}`;
                  return (
                    <div key={key} style={{
                      display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-field)',
                      padding: '0 var(--sp-3) 0 var(--sp-6)', borderBottom: '1px solid var(--border-weak)',
                      fontSize: 'var(--fs-5)', whiteSpace: 'nowrap',
                      ...marked(key === cur),
                    }}>
                      <span style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', overflow: 'hidden' }}>
                        <span className="k-sel" style={{ fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis' }}>{a.login}</span>
                        {key === cur && <Tag tone="ok">{t('ggs.accounts.current')}</Tag>}
                        {key === autoKey && <Tag tone="sub">{t('settings.account.at_start')}</Tag>}
                      </span>
                      <span style={{ width: 120, fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                        {a.last ? t('ggs.accounts.last', { when: whenFull(a.last) }) : ''}
                      </span>
                      <Button size="row" variant="ghost"
                              onClick={() => setDrop({ kind: 'account', addr: a.server, login: a.login })}>
                        {t('ggs.login.remove')}
                      </Button>
                    </div>
                  );
                })}
                <button type="button" className="k-row"
                        onClick={() => {
                          openNewLogin(sv.addr);
                          if (online) void ggsApi.disconnect();
                          onLogin();
                        }}
                        style={{ height: 'var(--h-field)', padding: '0 var(--sp-3) 0 var(--sp-6)', border: 0,
                                 borderBottom: '1px solid var(--border-weak)', textAlign: 'left', background: 'transparent',
                                 color: 'var(--sub)', fontSize: 'var(--fs-5)' }}>
                  {t('settings.account.add_here')}
                </button>
              </div>
            );
          })}
          <div style={{ paddingTop: 'var(--sp-3)' }}>
            <Button onClick={() => setEdit('new')}>{t('settings.server.add')}</Button>
          </div>
        </div>
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', padding: 'var(--sp-2) 0 0' }}>
          {t('settings.account.saved_help')}
        </span>
      </SettingGroup>

      {edit && <ServerDialog server={edit === 'new' ? null : edit} onClose={() => setEdit(null)} />}
      {drop && (
        <Confirm danger
                 title={drop.kind === 'server'
                   ? t('settings.server.remove_title', { name: serverName(servers, drop.addr) })
                   : t('settings.account.remove_title', { name: drop.login })}
                 body={drop.kind === 'server'
                   ? t('settings.server.remove_body')
                   : t('settings.account.remove_body', { server: serverName(servers, drop.addr), name: drop.login })}
                 ok={t('settings.remove_ok')}
                 onCancel={() => setDrop(null)}
                 onOk={() => {
                   const d = drop;
                   setDrop(null);
                   void (d.kind === 'server' ? ggsApi.forgetServer(d.addr) : ggsApi.forgetAccount(d.addr, d.login))
                     .then(changed);
                 }} />
      )}
    </>
  );
}

/** The logged-in account's profile text (GGS `info`) and its password. */
function ProfileRows({ snap }: { snap: GgsSnapshot }) {
  const login = snap.login;
  useEffect(() => { ggsApi.finger(login).catch(() => {}); }, [login]);
  const saved = snap.fingers[login]?.fields.find(([k]) => k === 'info')?.[1];
  const [info, setInfo] = useState<string | null>(null);
  const [err, setErr] = useState('');
  const [pw, setPw] = useState(false);
  const text = info ?? saved ?? '';
  const dirty = saved !== undefined && text.trim() !== saved;
  const save = () => {
    if (!dirty) return;
    void ggsApi.setInfo(text).then(() => { setErr(''); setInfo(null); markApplied(); })
      .catch((x) => setErr(tErr(x)));
  };
  return (
    <>
      <SettingRow label={t('settings.account.info')} error={err || undefined}>
        <span style={{ display: 'flex', gap: 'var(--sp-2)', width: '100%', maxWidth: 'var(--w-control)' }}>
          <span style={{ flex: 1, minWidth: 0, display: 'flex' }}>
            <TextField value={text} onChange={setInfo} placeholder={t('settings.account.info_placeholder')}
                       onEnter={save} />
          </span>
          <Button disabled={!dirty} onClick={save}>
            {t('settings.account.info_save')}
          </Button>
        </span>
      </SettingRow>
      <SettingRow label={t('settings.account.password')}>
        <Button onClick={() => setPw(true)}>{t('settings.account.password_change')}</Button>
      </SettingRow>
      {pw && <PasswordDialog passw={snap.passw} onClose={() => setPw(false)} />}
    </>
  );
}

/** Changes the server's password and the one saved on this Mac together. */
function PasswordDialog({ passw, onClose }: { passw: string; onClose: () => void }) {
  const [cur, setCur] = useState('');
  const [next, setNext] = useState('');
  const [again, setAgain] = useState('');
  const [wait, setWait] = useState<number | null>(null);
  const [err, setErr] = useState('');
  const outcome = wait !== null && passw.startsWith(`${wait}|`) ? passw.slice(String(wait).length + 1) : '';
  const busy = wait !== null && outcome === '';
  const bad = outcome === 'bad';
  useEffect(() => {
    if (outcome === 'ok') { markApplied(); onClose(); }
  }, [outcome, onClose]);
  useEffect(() => {
    if (!busy) return;
    const id = setTimeout(() => { setWait(null); setErr(t('settings.password.no_answer')); }, 15000);
    return () => clearTimeout(id);
  }, [busy]);
  const spaces = /\s/.test(next);
  const mismatch = again !== '' && again !== next;
  const ready = cur !== '' && next !== '' && next === again && !spaces && !busy;
  const submit = () => {
    if (!ready) return;
    const id = Date.now();
    setErr(''); setWait(id);
    ggsApi.changePassword(cur, next, id).catch((x) => { setWait(null); setErr(tErr(x)); });
  };
  return (
    <Overlay onClose={onClose}>
      <Modal title={t('settings.password.title')} onClose={onClose}
             actions={<>
               <span style={{ marginLeft: 'auto' }} />
               <Button size="field" variant="ghost" onClick={onClose}>{t('dialog.cancel')}</Button>
               <Button size="field" variant="primary" disabled={!ready} onClick={submit}>
                 {busy ? t('settings.password.changing') : t('settings.password.ok')}
               </Button>
             </>}>
        <Field label={t('settings.password.current')} error={bad ? t('settings.password.wrong') : undefined}>
          <TextField value={cur} onChange={(v) => { setCur(v); if (bad) setWait(null); }} password invalid={bad} />
        </Field>
        <Field label={t('settings.password.new')} help={t('settings.password.new_help')}
               error={spaces ? t('settings.password.spaces') : undefined}>
          <TextField value={next} onChange={setNext} password invalid={spaces} />
        </Field>
        <Field label={t('settings.password.again')} error={mismatch ? t('settings.password.mismatch') : undefined}>
          <TextField value={again} onChange={setAgain} password invalid={mismatch} onEnter={submit} />
        </Field>
        {err && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--bad)' }}>{err}</span>}
      </Modal>
    </Overlay>
  );
}

/** Adds or edits a server: a name, its address and whether games there teach the book. */
export function ServerDialog({ server, onClose }: { server: SavedServer | null; onClose: () => void }) {
  const [name, setName] = useState(server?.name ?? '');
  const [addr, setAddr] = useState(server?.addr ?? '');
  const [pub, setPub] = useState(server?.public ?? false);
  const [err, setErr] = useState('');
  const save = async () => {
    try {
      await ggsApi.saveServer(server?.addr ?? '', name, addr, pub);
      changed();
      markApplied();
      onClose();
    } catch (e) { setErr(tErr(e)); }
  };
  return (
    <Overlay onClose={onClose}>
      <Modal title={server ? t('settings.server.edit_title') : t('settings.server.add_title')} onClose={onClose}
             actions={<>
               <span style={{ flex: 1, minWidth: 0, fontSize: 'var(--fs-6)', color: 'var(--bad)' }}>{err}</span>
               <Button size="field" variant="ghost" onClick={onClose}>{t('dialog.cancel')}</Button>
               <Button size="field" variant="primary" disabled={!addr.trim()} onClick={() => void save()}>{t('settings.server.save')}</Button>
             </>}>
        <Field label={t('settings.server.name')}>
          <TextField value={name} onChange={setName} />
        </Field>
        <Field label={t('settings.server.addr')}>
          <TextField value={addr} onChange={setAddr} mono placeholder={t('settings.server.addr_placeholder')} />
        </Field>
        <Field label={t('settings.server.kind')} help={t('settings.server.kind_help')}>
          <Segmented value={pub ? 'public' : 'test'} onChange={(v) => setPub(v === 'public')} fill
                     options={[{ value: 'public', label: t('settings.server.public') },
                               { value: 'test', label: t('settings.server.test') }]} />
        </Field>
      </Modal>
    </Overlay>
  );
}

const validLogChannel = (c: string) => c === '' || /^\.[A-Za-z0-9_-]{1,7}$/.test(c);

function GgsPlaySettings({ snap, learn, setLearn }: { snap: GgsSnapshot; learn: boolean; setLearn: (on: boolean) => void }) {
  const e = snap.engine;
  const [auto, setAuto] = useState(snap.auto_play);
  const [watch, setWatch] = useState(snap.watch_analysis);
  const [ponder, setPonder] = useState(snap.ponder);
  const [logChannel, setLogChannel] = useState(snap.log_channel);
  const logChannelOk = validLogChannel(logChannel);
  const [book, setBook] = useState(e.use_book);
  const [err, setErr] = useState('');

  const first = useRef(true);
  useEffect(() => {
    if (first.current) { first.current = false; return; }
    const timer = setTimeout(() => void (async () => {
      try {
        await ggsApi.setAutoPlay(auto);
        await ggsApi.setWatchAnalysis(watch);
        await ggsApi.setPonder(ponder);
        if (logChannelOk) await ggsApi.setLogChannel(logChannel);
        await ggsApi.setUseBook(book);
        setErr('');
        markApplied();
      } catch (x) { setErr(t('ggs.settings.apply_failed', { error: tErr(x) })); }
    })(), 400);
    return () => clearTimeout(timer);
  }, [auto, watch, ponder, logChannel, book, logChannelOk]);

  return (
    <SettingGroup title={t('settings.cat.play')}>
      <SettingRow label={t('ggs.book')} error={err || undefined}
                  help={e.book_loaded ? t('settings.play.book_help') : t('ggs.settings.book_missing')}>
        {e.book_loaded ? (
          <SettingToggle checked={book} onChange={setBook} />
        ) : (
          <Button onClick={async () => {
            const p = await api.pickResource('book');
            if (p) { await api.setResource('book', p); emitApp('resources-changed'); }
          }}>{t('settings.play.pick_book')}</Button>
        )}
      </SettingRow>
      <SettingRow label={t('ggs.settings.learn')}>
        <SettingToggle checked={learn} onChange={setLearn} />
      </SettingRow>
      <SettingRow label={t('ggs.settings.auto_play')}>
        <SettingToggle checked={auto} onChange={setAuto} />
      </SettingRow>
      <SettingRow label={t('ggs.settings.ponder')}>
        <SettingToggle checked={ponder} onChange={setPonder} />
      </SettingRow>
      <SettingRow label={t('ggs.settings.watch_analysis')}>
        <SettingToggle checked={watch} onChange={setWatch} />
      </SettingRow>
      <SettingRow label={t('ggs.settings.log_channel')} help={t('ggs.settings.log_channel_note')}
                  error={logChannelOk ? undefined : t('ggs.settings.log_channel_invalid')}>
        <TextField mono width={120} value={logChannel} onChange={setLogChannel} invalid={!logChannelOk} />
      </SettingRow>
    </SettingGroup>
  );
}

const flat = (x: string) => x.replace(/\s+/g, '');

function GgsFormSettings({ snap }: { snap: GgsSnapshot }) {
  const login = snap.login;
  useEffect(() => { if (login) ggsApi.finger(login).catch(() => {}); }, [login]);
  const form = (key: string) =>
    (snap.fingers[login]?.fields.find(([k]) => fingerKey(k) === key)?.[1] ?? '')
      .replace(/^\s*:\s*/, '').trim();
  const [err, setErr] = useState('');
  // Sent but not yet seen in finger: GGS answers a bad formula with "dform ERR …" and keeps the old one.
  const [pending, setPending] = useState<{ kind: 'aform' | 'dform'; expr: string } | null>(null);
  const save = async (kind: 'aform' | 'dform', expr: string) => {
    try {
      await ggsApi.setFormula(kind, expr);
      setErr('');
      setPending({ kind, expr });
    } catch (e) {
      setErr(t('ggs.settings.formula_failed', { error: tErr(e) }));
      return;
    }
    if (login) ggsApi.finger(login).catch(() => {});
  };
  const now = pending ? form(pending.kind === 'aform' ? 'accept' : 'decline') : '';
  const [refusedKind, refusedWhy] = snap.formula_refused.split(/\|(.*)/s);
  const refused = !!pending && refusedKind === pending.kind;
  const done = !!pending && !refused && flat(now) === flat(pending.expr);
  useEffect(() => { if (done) markApplied(); }, [done]);
  return (
    <>
    <SettingGroup title={t('settings.cat.form')} aside={t('settings.form.aside')}>
      {(refused || err) && (
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--bad)' }}>
          {refused ? t('ggs.settings.formula_refused', { error: refusedWhy ?? '' }) : err}
        </span>
      )}
      <FormulaField key={'a:' + form('accept')} label={t('ggs.finger.accept')} src={form('accept')}
                    onSave={(x) => void save('aform', x)} />
      <FormulaField key={'d:' + form('decline')} label={t('ggs.finger.decline')} src={form('decline')}
                    onSave={(x) => void save('dform', x)}
                    preset={{ src: DEFAULT_DECLINE, label: t('ggs.settings.default_decline'),
                              note: t('ggs.settings.default_decline_note') }} />
    </SettingGroup>
    <SettingGroup title={t('settings.form.open_requests')}>
      <SettingRow label={t('settings.form.open_requests_show')}>
        <SettingToggle checked={snap.open_requests}
                       onChange={(on) => void ggsApi.setOpenRequests(on).then(markApplied)} />
      </SettingRow>
    </SettingGroup>
    </>
  );
}
