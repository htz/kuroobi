import { useState } from 'react';
import { Button, Segmented, Select, Toggle } from './components/primitives';
import { Dock, KeyValue, Note, Section } from './components/layout';
import { KifuTable } from './components/data';
import { Strength, presetOf } from './components/strength';
import { api } from './api';
import { clockChoices, fmtSecs } from './ggs';
import type { Prefs } from './prefs';
import type { Move } from './components/data';
import type { Game } from './state';
import { t, tErr } from './i18n';

// The same clocks as a GGS request ("00:05:00" there, seconds here).
const clockOptions = (): [string, string][] => clockChoices().map(([hms, label]) => [
  String(hms.split(':').reduce((a, n) => a * 60 + Number(n), 0)), label,
]);

/**
 * The right panel of play and study, one page: the strength on top (study: the analysis settings),
 * the record below. Learning lives in the engine settings.
 */
export function PlayDock({
  g, prefs, setClockSecs, open, onPaste, onLoadFile, study, moves, ggfNames,
}: {
  g: Game;
  prefs: Prefs;
  setClockSecs: (secs: number) => void;
  open: boolean;
  onPaste: () => void;
  onLoadFile: () => void;
  study: boolean;
  moves: Move[];
  ggfNames: () => [string, string];
}) {
  return (
    <Dock tabs={[{ id: 'main', label: '' }]} active="main" open={open} scroll={false}>
      <div style={{ flex: 'none', paddingTop: 'var(--sp-3)', borderBottom: '1px solid var(--border)' }}>
        <Section title={study ? t('dock.strength.analysis_title') : t('dock.strength.title')}>
          {study ? <LevelOnly g={g} /> : <LevelOrClock g={g} prefs={prefs} setClockSecs={setClockSecs} />}
          {/* The same row as every switch: words on the left, the switch on the right. */}
          <Toggle checked={g.useBook} disabled={!g.hasBook} onChange={g.setUseBook} label={t('dock.book.label')} />
          {!g.hasBook && <Note>{t('dock.book.missing')}</Note>}
        </Section>
      </div>
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        <KifuTable moves={moves} current={g.view?.cursor} decimals={prefs.decimals}
                   onSelect={(n) => void g.jumpTo(n)} />
      </div>
      <div style={{
        flex: 'none', display: 'flex', gap: 'var(--sp-2)',
        padding: 'var(--sp-2) var(--sp-3)', borderTop: '1px solid var(--border-weak)',
      }}>
        {!study && <Button title="⌘O" onClick={() => onPaste()}>{t('dock.record.paste')}</Button>}
        {/* Study loads a record from its toolbar (「棋譜を読み込む」, which also takes a file). */}
        {!study && <Button onClick={() => void onLoadFile()}>{t('dock.record.load')}</Button>}
        <Button title="⌘S" disabled={!moves.length}
                onClick={() => void api.saveKifu(...ggfNames()).catch((e: unknown) => g.say(tErr(e)))}>
          {t('dock.record.save')}
        </Button>
      </div>
    </Dock>
  );
}

function LevelOnly({ g }: { g: Game }) {
  return (
    <Strength value={g.levels} onChange={(v) => {
      const i = presetOf(v);
      if (i !== 'custom') { g.setLevel(i); return; }
      g.setCustom(v);
      g.setLevel('custom');
    }} />
  );
}

/**
 * KUROOBI plays to a level or to a clock (then it spends its own time, and the person gets the same).
 * Once a move is on the board the choice and the clock stay as they are until a new game; a clock
 * game shows both times left in the clock's place.
 */
function LevelOrClock({ g, prefs, setClockSecs }: {
  g: Game; prefs: Prefs; setClockSecs: (secs: number) => void;
}) {
  const byClock = prefs.clockSecs > 0;
  const [lastClock, setLastClock] = useState(byClock ? prefs.clockSecs : 600);
  // Started once the record has a move (also when looking back at its start).
  const started = (g.view?.moves.length ?? 0) > 0;
  const who = (color: 'black' | 'white') =>
    g.side === 'both' || g.side === color ? t('dock.strength.kuroobi') : t('dock.strength.you');
  const left = (color: 'black' | 'white') =>
    t('dock.strength.left', { color: t(color === 'black' ? 'app.color.black' : 'app.color.white'), who: who(color) });
  return (
    <>
      <Segmented fill disabled={started} value={byClock ? 'clock' : 'level'}
                 onChange={(v) => setClockSecs(v === 'clock' ? lastClock : 0)}
                 options={[{ value: 'level', label: t('dock.strength.by_level') },
                           { value: 'clock', label: t('dock.strength.by_clock') }]} />
      {!byClock && <LevelOnly g={g} />}
      {byClock && !started && (
        <Select value={String(prefs.clockSecs)}
                onChange={(v) => { setLastClock(+v); setClockSecs(+v); }}
                options={clockOptions()} />
      )}
      {byClock && started && (['black', 'white'] as const).map((c) => (
        <KeyValue key={c} big label={left(c)} value={fmtSecs(g.clock?.[c] ?? prefs.clockSecs)} />
      ))}
    </>
  );
}
