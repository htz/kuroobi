import { useMemo, useState } from 'react';
import { t, dateLocale } from '../i18n';
import { Button, TextField } from './primitives';
import { picked } from './layout';
import { Tag } from './ggs';

export type Account = { server: string; login: string; last?: number };

export const PUBLIC_SERVER = 'skatgame.net:5000';

export const accountKey = (a: Account): string => `${a.server}|${a.login}`;

export const lastUsed = (secs?: number): string =>
  secs ? new Date(secs * 1000).toLocaleString(dateLocale(), { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : '';

/** Saved logins as a list pane: grouped by server, most recent first, searchable once it grows. */
export function AccountList({ accounts, selected, current, onSelect, onNew }: {
  accounts: Account[];
  selected?: string;
  current?: string;
  onSelect: (a: Account) => void;
  onNew?: () => void;
}) {
  const [q, setQ] = useState('');
  const needle = q.trim().toLowerCase();
  const shown = useMemo(() => accounts
    .filter((a) => !needle || a.login.toLowerCase().includes(needle) || a.server.toLowerCase().includes(needle))
    .sort((a, b) => (b.last ?? 0) - (a.last ?? 0)), [accounts, needle]);
  const servers = [...new Set(shown.map((a) => a.server))];

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div style={{
        flex: 'none', height: 'var(--h-field)', display: 'flex', alignItems: 'center',
        gap: 'var(--sp-2)', padding: '0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
      }}>
        <span style={{ fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)', whiteSpace: 'nowrap' }}>
          {t('ggs.accounts.title')}
        </span>
        <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
          {needle ? t('ggs.accounts.count', { shown: shown.length, all: accounts.length }) : accounts.length}
        </span>
        <span style={{ marginLeft: 'auto' }} />
        {onNew && <Button size="chip" onClick={onNew}>{t('ggs.accounts.new')}</Button>}
      </div>
      {accounts.length > 4 && (
        <div style={{ flex: 'none', padding: 'var(--sp-2) var(--sp-3)', borderBottom: '1px solid var(--border-weak)' }}>
          <TextField value={q} onChange={setQ} placeholder={t('ggs.accounts.search')}
                     onEnter={() => { if (shown[0]) onSelect(shown[0]); }} />
        </div>
      )}
      <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>
        {!shown.length && (
          <div style={{ padding: 'var(--sp-3)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('ggs.accounts.none_match')}</div>
        )}
        {servers.map((srv) => (
          <div key={srv}>
            <div style={{
              position: 'sticky', top: 0, zIndex: 1, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
              padding: '4px var(--sp-3)', background: 'var(--bg)', borderBottom: '1px solid var(--border-weak)',
              fontSize: 'var(--fs-7)', color: 'var(--sub)',
            }}>
              <span style={{ fontFamily: 'var(--ff-mono)' }}>{srv}</span>
              {srv === PUBLIC_SERVER && <Tag tone="accent">{t('ggs.accounts.public')}</Tag>}
            </div>
            {shown.filter((a) => a.server === srv).map((a) => {
              const k = accountKey(a);
              const on = k === selected;
              return (
                <button key={k} type="button" onClick={() => onSelect(a)}
                  aria-current={on || undefined} className={'k-row' + (on ? ' k-on' : '')}
                  style={{
                    width: '100%', border: 0, textAlign: 'left', display: 'flex', alignItems: 'center',
                    gap: 'var(--sp-2)', padding: 'var(--sp-2) var(--sp-3)', fontSize: 'var(--fs-5)',
                    borderBottom: '1px solid var(--border-weak)', color: 'var(--text)', ...picked(on),
                  }}>
                  <span style={{ fontWeight: 600 }}>{a.login}</span>
                  {k === current && <Tag tone="ok">{t('ggs.accounts.current')}</Tag>}
                  <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{lastUsed(a.last)}</span>
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}
