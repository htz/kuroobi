import { useState } from 'react';
import type { GgsSnapshot } from '../types';
import { emitApp, ggsApi } from '../api';
import { Confirm } from '../Dialogs';
import { t, tErr } from '../i18n';
import { Caption, Field, ListRow, Toolbar } from '../components/layout';
import { Button, Select, TextField } from '../components/primitives';
import { Tag } from '../components/ggs';
import { PUBLIC_SERVER } from '../components/accounts';
import { ServerDialog, useServers, serverName } from '../SettingsScreen';
import { ListHead, ListPane, Page, TitleBit, act } from '../ggsui';
import { day, whenFull } from '../when';

const ADD = '__add';

/** "10/25": in the list, the day is enough; the detail shows the time too. */

let wantNew: string | null = null;
/** Opens the login screen on the new-account form the next time it shows, on `server` when given. */
export const openNewLogin = (server = '') => { wantNew = server; };

/** Saved accounts grouped by server on the left; the chosen one, or a new login, on the right. */
export function Login({ conn, server: linkingTo }: { conn: GgsSnapshot['conn']; server: string }) {
  const { servers, accounts } = useServers();
  const [asked] = useState(() => { const w = wantNew; wantNew = null; return w; });
  const [chosen, setSel] = useState<string | null>(asked !== null ? '' : null);   // "server|login", '' = new account
  const [user, setUser] = useState('');
  const [pw, setPw] = useState('');
  const [server, setServer] = useState(asked || PUBLIC_SERVER);
  const [adding, setAdding] = useState(false);
  const [status, setStatus] = useState('');
  const [busy, setBusy] = useState(false);
  const [drop, setDrop] = useState<{ server: string; login: string } | null>(null);


  const run = async (go: () => Promise<void>, addr: string) => {
    setStatus(t('login.connecting_to', { server: addr }));
    setBusy(true);
    try { await go(); setStatus(''); } catch (e) { setStatus(tErr(e)); }
    setBusy(false);
  };
  const connect = () => run(() => ggsApi.connect(user, pw, server === PUBLIC_SERVER ? '' : server), server);
  // Until something is chosen, the most recently used account is.
  const recent = [...accounts].sort((x, y) => (y.last ?? 0) - (x.last ?? 0))[0];
  const sel = chosen ?? (recent ? `${recent.server}|${recent.login}` : null);
  const picked = accounts.find((a) => `${a.server}|${a.login}` === sel);
  const first = !accounts.length;
  const showForm = first || sel === '' || !picked;
  const groups = servers.map((s) => ({ s, list: accounts.filter((a) => a.server === s.addr) })).filter((g) => g.list.length);
  const pub = (addr: string) => servers.find((s) => s.addr === addr)?.public ?? addr === PUBLIC_SERVER;
  // The command returns at once; the connection itself shows up in the snapshot until it is online or gives up.
  const linking = conn === 'connecting' || conn === 'logging_in';
  const working = busy || linking;
  const shown = linking ? t('login.connecting_to', { server: linkingTo || PUBLIC_SERVER }) : status;
  const msg = (
    <span style={{ fontSize: 'var(--fs-6)', minHeight: '1.5em', color: working ? 'var(--sub)' : 'var(--bad)' }}>{shown}</span>
  );

  return (
    <>
      <Toolbar><TitleBit title={t('ggs.login.title')} /></Toolbar>
      <Page row>
        {!first && (
          <ListPane head={
            <div style={{ flex: 'none', height: 'var(--h-field)', display: 'flex', alignItems: 'center', padding: '0 var(--sp-3)',
                          borderBottom: '1px solid var(--border-weak)' }}><Caption>{t('ggs.login.saved')}</Caption></div>
          } footRow foot={
            <ListRow on={sel === ''} rule={false} onClick={() => { setSel(''); setStatus(''); }}>
              {t('login.new_row')}
            </ListRow>
          }>
            {groups.map(({ s, list }) => (
              <div key={s.addr} style={{ display: 'flex', flexDirection: 'column' }}>
                <ListHead>{s.name}<span style={{ fontFamily: 'var(--ff-mono)', fontWeight: 400, letterSpacing: 0 }}>{s.addr}</span></ListHead>
                {list.map((a) => {
                  const key = `${a.server}|${a.login}`;
                  const on = key === sel;
                  return (
                    <ListRow key={key} on={on} onClick={() => { setSel(key); setStatus(''); }}>
                      <b style={{ fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis' }}>{a.login}</b>
                      <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{a.last ? day(a.last) : ''}</span>
                    </ListRow>
                  );
                })}
              </div>
            ))}
          </ListPane>
        )}
        <div className="k-scroll" style={{ flex: 1, minWidth: 0, minHeight: 0, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)' }}>
          {!showForm && picked ? (
            <div style={{ width: 'var(--w-modal)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                <span className="k-sel" style={{ fontSize: 'var(--fs-1)', fontWeight: 700 }}>{picked.login}</span>
                <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
                  <span style={{ fontSize: 'var(--fs-5)' }}>{serverName(servers, picked.server)}</span>
                  <span className="k-sel" style={{ fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{picked.server}</span>
                  {pub(picked.server) && <Tag tone="accent">{t('settings.server.public')}</Tag>}
                </span>
                {picked.last ? <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('ggs.accounts.last', { when: whenFull(picked.last) })}</span> : null}
              </div>
              <Button variant="primary" size="field" disabled={working}
                      onClick={() => void run(() => ggsApi.connectSaved(picked.server, picked.login), picked.server)}>
                {working ? t('ggs.login.connecting') : t('ggs.login.submit')}
              </Button>
              {msg}
              <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
                <Button variant="ghost" onClick={() => setDrop(picked)}>{t('login.remove')}</Button>
              </div>
            </div>
          ) : (
            <div style={{ width: 'var(--w-modal)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
              <span style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{first ? t('ggs.login.title') : t('login.new')}</span>
              <Field label={t('ggs.login.server')}>
                <Select value={server} onChange={(v) => (v === ADD ? setAdding(true) : setServer(v))}
                        options={[...servers.map((s) => [s.addr, `${s.name} · ${s.addr}`] as [string, string]),
                                  [ADD, t('login.add_server')]]} />
              </Field>
              <Field label={t('ggs.login.username')}>
                <TextField value={user} onChange={setUser} />
              </Field>
              <Field label={t('ggs.login.password')}>
                <TextField value={pw} password onChange={setPw} onEnter={() => void connect()} invalid={!!status && !working} />
              </Field>
              <Button variant="primary" size="field" disabled={working || !user.trim()} onClick={() => void connect()}>
                {working ? t('ggs.login.connecting') : t('ggs.login.submit')}
              </Button>
              {msg}
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('login.saved_help')}</span>
            </div>
          )}
        </div>
      </Page>
      {adding && <ServerDialog server={null} onClose={() => setAdding(false)} />}
      {drop && (
        <Confirm danger title={t('settings.account.remove_title', { name: drop.login })}
                 body={t('settings.account.remove_body', { server: serverName(servers, drop.server), name: drop.login })}
                 ok={t('settings.remove_ok')}
                 onCancel={() => setDrop(null)}
                 onOk={() => {
                   const d = drop;
                   setDrop(null);
                   setSel(null);
                   act(ggsApi.forgetAccount(d.server, d.login).then(() => emitApp('servers-changed')));
                 }} />
      )}
    </>
  );
}
