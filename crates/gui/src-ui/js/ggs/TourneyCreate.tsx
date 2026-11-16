import { useEffect, useState } from 'react';
import { ggsApi } from '../api';
import type { GgsSnapshot } from '../types';
import { askClockChoices, clockSpec, gtypeChoices, MIN_CLOCK, parseClockSpec } from '../ggs';
import { t } from '../i18n';
import { Field, Modal, Overlay } from '../components/layout';
import { Button, ClockField, Segmented, Select, TextField, Toggle } from '../components/primitives';
import { registered } from '../AskSheet';
import { act } from '../ggsui';

const td = (cmd: string) => act(ggsApi.raw(`tell /td ${cmd}`));

// The director's own bounds (Service/TD: HandleCmdNew).
const MAX_ROUNDS = 10000;
const MAX_START = 30;

const whole = (v: string, lo: number, hi: number): number | null => {
  if (!/^\d+$/.test(v.trim())) return null;
  const n = Number(v);
  return n >= lo && n <= hi ? n : null;
};

export function TourneyCreate({ snap, onClose, onCreated }: {
  snap: GgsSnapshot; onClose: () => void; onCreated: (id: number) => void;
}) {
  const reg = registered(snap);
  const clocks = askClockChoices();
  const [style, setStyle] = useState<'r' | 's'>('r');
  const [gtype, setGtype] = useState('s8r14');
  const init = parseClockSpec(clocks[2]?.[0] ?? '00:05:00') ?? { main: 300, ext: 120 };
  const [mainSecs, setMainSecs] = useState(init.main);
  const [extSecs, setExtSecs] = useState(init.ext);
  const time = clockSpec(mainSecs, extSecs);
  const [customClock, setCustomClock] = useState(false);
  const [rounds, setRounds] = useState('10');
  const [start, setStart] = useState('10');
  const [join, setJoin] = useState(reg);
  // Unix seconds the command went out; the director's next `new` reply after it is ours.
  const [sentAt, setSentAt] = useState<number | null>(null);

  const nRounds = whole(rounds, 1, MAX_ROUNDS);
  const nStart = whole(start, 1, MAX_START);
  const reply = snap.td.reply;
  const got = sentAt != null && reply && reply.cmd === 'new' && reply.at >= sentAt ? reply : null;
  const waiting = sentAt != null && !got;
  const createdId = got && !got.err ? Number(/tournament (\d+)/.exec(got.text)?.[1] ?? NaN) : NaN;

  useEffect(() => {
    if (!Number.isFinite(createdId)) return;
    if (join && reg) td(`join ${createdId}`);
    td('t');
    onCreated(createdId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [createdId]);

  const go = () => {
    if (nRounds == null || nStart == null) return;
    setSentAt(Math.floor(Date.now() / 1000) - 1);
    td(`new ${style} /os ${gtype} ${time} ${nRounds} ${nStart}`);
  };

  return (
    <Overlay onClose={onClose}>
      <Modal title={t('tourney.create.title')} onClose={onClose}
             actions={<>
               <span style={{ flex: 1, minWidth: 0, fontSize: 'var(--fs-6)', lineHeight: 1.35, color: 'var(--bad)' }}>
                 {got?.err ? t('tourney.create.refused', { text: got.text }) : ''}
               </span>
               <Button size="field" variant="ghost" onClick={onClose}>{t('dialog.cancel')}</Button>
               <Button size="field" variant="primary" disabled={nRounds == null || nStart == null || waiting} onClick={go}>
                 {waiting ? t('tourney.create.creating') : t('tourney.create.go')}
               </Button>
             </>}>
        <Field label={t('tourney.pairing')}>
          <Segmented value={style} onChange={setStyle} fill
                     options={[{ value: 'r' as const, label: t('tourney.create.rrobin') },
                               { value: 's' as const, label: t('tourney.create.swiss') }]} />
        </Field>
        <Field label={t('ggs.field.format')}>
          <Select value={gtype} onChange={setGtype} options={gtypeChoices()} />
        </Field>
        <Field label={t('ggs.field.time_control')}>
          <Select value={customClock ? 'custom' : time}
                  onChange={(v) => {
                    setCustomClock(v === 'custom');
                    const c = v === 'custom' ? null : parseClockSpec(v);
                    if (c) { setMainSecs(c.main); setExtSecs(c.ext); }
                  }}
                  options={[...clocks, ['custom', t('ggs.clock_choice.custom')]]} />
          {customClock && (
            <span style={{ display: 'flex', alignItems: 'center', flexWrap: 'wrap', gap: 'var(--sp-2) var(--sp-4)', whiteSpace: 'nowrap' }}>
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
                {t('ggs.field.time_control')}
                <ClockField value={mainSecs} onChange={setMainSecs} min={MIN_CLOCK} label={t('ggs.field.time_control')} />
              </span>
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
                {t('ggs.field.extension')}
                <ClockField value={extSecs} onChange={setExtSecs} label={t('ggs.field.extension')} />
              </span>
            </span>
          )}
        </Field>
        <Field label={t('tourney.create.rounds')} help={style === 'r' ? t('tourney.create.rounds_help') : undefined}
               error={nRounds == null ? t('tourney.create.range', { lo: 1, hi: MAX_ROUNDS }) : undefined}>
          <TextField value={rounds} onChange={setRounds} numeric align="right" width={80} invalid={nRounds == null} />
        </Field>
        <Field label={t('tourney.create.start')} help={t('tourney.create.start_help', { hi: MAX_START })}
               error={nStart == null ? t('tourney.create.range', { lo: 1, hi: MAX_START }) : undefined}>
          <TextField value={start} onChange={setStart} numeric align="right" width={80} invalid={nStart == null} />
        </Field>
        <span style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', opacity: reg ? 1 : 0.4 }}>
          <span style={{ display: 'flex', alignItems: 'center', height: 'var(--h-ctrl)' }}>
            <Toggle checked={join && reg} onChange={(v) => reg && setJoin(v)} label={t('tourney.create.join')} />
          </span>
          {!reg && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('tourney.create.unregistered')}</span>}
        </span>
      </Modal>
    </Overlay>
  );
}
