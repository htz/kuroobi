import { useRef, useState } from 'react';
import { ggsApi } from '../api';
import type { GgsSnapshot } from '../types';
import { logLinesOf } from '../adapt';
import { t, tErr } from '../i18n';
import { Divider, FlashNote, ListRow, popoverStyle, Toolbar, TwoLines, useDismiss, useFlash } from '../components/layout';
import { Button, Segmented, TextField, Toggle } from '../components/primitives';
import { ConsoleLog } from '../components/ggs';
import { Blank, Page, act } from '../ggsui';

// Commands people type often; picking one puts it in the input, where names can be filled in.
const COMMANDS: [string, string][] = [
  ['tell /os who 8', 'console.cmd.who'],
  ['tell /os match', 'console.cmd.match'],
  ['tell /os stored', 'console.cmd.stored'],
  ['tell /os finger ', 'console.cmd.finger'],
  ['tell /os history ', 'console.cmd.history'],
  ['tell /os rank 8r ', 'console.cmd.rank'],
  ['tell /os top 8r 20', 'console.cmd.top'],
  ['tell /os look .', 'console.cmd.look'],
  ['tell /os watch + .', 'console.cmd.watch'],
  ['tell /os watch - .', 'console.cmd.unwatch'],
  ['tell /os ask s8r14 05:00 ', 'console.cmd.ask'],
  ['tell /os rated +', 'console.cmd.rated'],
];

export function Console({ snap }: { snap: GgsSnapshot }) {
  const [cmd, setCmd] = useState('');
  const [dir, setDir] = useState<'all' | 'out' | 'in'>('all');
  const [query, setQuery] = useState('');
  const [follow, setFollow] = useState(true);
  const [from, setFrom] = useState(0);
  const [cmds, setCmds] = useState(false);
  const cmdsBox = useRef<HTMLDivElement>(null);
  const cmdsBtn = useRef<HTMLSpanElement>(null);
  useDismiss(cmds, () => setCmds(false), cmdsBox, cmdsBtn);
  const [note, flash] = useFlash();
  const say = (msg: string) => flash(msg, true);
  const send = () => {
    const c = cmd.trim();
    if (!c) return;
    act(ggsApi.raw(c));
    setCmd('');
  };
  const all = logLinesOf(snap.log);
  const q = query.toLowerCase();
  const shown = all.filter((l) => l.seq >= from && (dir === 'all' || l.dir === dir) && (!q || l.text.toLowerCase().includes(q)));
  const text = () => shown.map((l) => (l.dir === 'out' ? '› ' : '') + l.text).join('\n') + '\n';

  return (
    <>
      <Toolbar end={<>
        <FlashNote note={note} />
        <Button disabled={!shown.length} onClick={() => {
          void navigator.clipboard.writeText(text()).catch(() => say(t('ggs.console.copy_failed')));
        }}>{t('ggs.console.copy')}</Button>
        <Button disabled={!shown.length}
                onClick={() => void ggsApi.saveLog(text()).catch((e) => say(t('ggs.console.save_failed', { error: tErr(e) })))}>
          {t('ggs.console.save')}
        </Button>
        <Button variant="ghost" disabled={!all.length} onClick={() => setFrom((all.at(-1)?.seq ?? -1) + 1)}>{t('ggs.console.clear')}</Button>
      </>}>
        <Segmented value={dir} onChange={setDir} options={[
          { value: 'all', label: t('ggs.filter.all') },
          { value: 'out', label: t('ggs.console.dir_out') },
          { value: 'in', label: t('ggs.console.dir_in') },
        ]} />
        <TextField value={query} onChange={setQuery} placeholder={t('console.search')} width={220} />
        <Divider />
        <Toggle checked={follow} onChange={setFollow} label={t('console.follow')} />
      </Toolbar>
      <Page>
        <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
          {all.length ? <ConsoleLog lines={shown} follow={follow} /> : <Blank title={t('console.empty')} />}
        </div>
        <div style={{ position: 'relative', flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                      padding: 'var(--sp-2) var(--sp-4)', borderTop: '1px solid var(--border)', background: 'var(--panel)' }}>
          {cmds && (
            <div ref={cmdsBox} role="dialog" aria-label={t('console.commands')} style={{
              position: 'absolute', zIndex: 2, right: 'var(--sp-4)', bottom: 'calc(100% + var(--sp-1))',
              width: 'var(--w-popover)', ...popoverStyle,
            }}>
              <div className="k-scroll" style={{ maxHeight: 280, padding: 'var(--sp-1) 0' }}>
                {COMMANDS.map(([c, what]) => (
                  <ListRow key={c} height="row2" rule={false} onClick={() => { setCmd(c); setCmds(false); }}>
                    <TwoLines top={<span style={{ fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)' }}>{c}</span>} bottom={t(what)} />
                  </ListRow>
                ))}
              </div>
            </div>
          )}
          <span style={{ flex: 'none', fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-5)', color: 'var(--accent)' }}>›</span>
          <span style={{ flex: 1, minWidth: 0, display: 'flex' }}>
            <TextField mono value={cmd} onChange={setCmd} onEnter={send} placeholder={t('ggs.console.placeholder')} />
          </span>
          <span ref={cmdsBtn} style={{ display: 'flex' }}>
            <Button size="field" onClick={() => setCmds(!cmds)}>{t('console.commands')} {cmds ? '▾' : '▴'}</Button>
          </span>
          <Button size="field" variant="primary" onClick={send}>{t('ggs.send')}</Button>
        </div>
      </Page>
    </>
  );
}
