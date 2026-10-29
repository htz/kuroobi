import { useCallback, useEffect, useRef, useState } from 'react';
import { api, emitApp, ggsApi, onApp, type SavedServer } from './api';
import type { GgsSnapshot } from './types';
import type { Prefs } from './prefs';
import type { Account } from './components/accounts';
import { DEFAULT_DECLINE, fingerKey } from './ggs';
import { t, tErr, useLang } from './i18n';
import { Confirm, Field, ListRow, marked, Modal, Overlay } from './components/layout';
import { ListHead } from './ggsui';
import { Button, Segmented, Select, TextField } from './components/primitives';
import { Tag } from './components/ggs';
import { EngineSettings, SettingGroup, SettingRow, SettingToggle, ViewSettings, type LearnSetting } from './Dialogs';
import { FormulaField } from './GgsScreens';
import { openNewLogin } from './ggs/Login';
import { markApplied } from './applied';
import { whenFull } from './when';

export type SettingsCat = 'engine' | 'view' | 'ggs-account' | 'ggs-play' | 'ggs-form';

const groups = (): { title: string; items: [SettingsCat, string][] }[] => [
  { title: t('settings.group.app'), items: [['engine', t('settings.tab.engine')], ['view', t('settings.tab.view')]] },
  {
    title: 'GGS', items: [
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
          {cat === 'ggs-account' && <GgsAccountSettings snap={snap} onLogin={onLogin} />}
          {cat === 'ggs-play' && (snap && snap.conn === 'online'
            ? <GgsPlaySettings snap={snap} /> : <Offline />)}
          {cat === 'ggs-form' && (snap && snap.conn === 'online'
            ? <GgsFormSettings snap={snap} /> : <Offline />)}
        </div>
      </div>
    </div>
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
      <SettingGroup title={t('settings.account.current')}>
        <SettingRow label={t('settings.account.account')}
                    help={online ? t('settings.account.logout_help') : undefined}>
          {online ? <>
            <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-ctrl)' }}>
              <b className="k-sel" style={{ fontWeight: 600 }}>{snap!.login}</b>
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{serverName(servers, snap!.server)}</span>
              <span className="k-sel" style={{ fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{snap!.server}</span>
            </span>
            <Button variant="danger" onClick={() => void ggsApi.disconnect().then(changed)}>{t('ggs.settings.logout')}</Button>
          </> : <span style={{ lineHeight: 'var(--h-ctrl)', color: 'var(--sub)' }}>{t('settings.account.none')}</span>}
        </SettingRow>
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

function GgsPlaySettings({ snap }: { snap: GgsSnapshot }) {
  const e = snap.engine;
  const [auto, setAuto] = useState(snap.auto_play);
  const [watch, setWatch] = useState(snap.watch_analysis);
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
        if (logChannelOk) await ggsApi.setLogChannel(logChannel);
        await ggsApi.setUseBook(book);
        setErr('');
        markApplied();
      } catch (x) { setErr(t('ggs.settings.apply_failed', { error: tErr(x) })); }
    })(), 400);
    return () => clearTimeout(timer);
  }, [auto, watch, logChannel, book, logChannelOk]);

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
      <SettingRow label={t('ggs.settings.auto_play')}>
        <SettingToggle checked={auto} onChange={setAuto} />
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
  );
}
