import { Button, Progress, Segmented, Toggle } from './components/primitives';
import { Busy, Dock, KeyValue, Note, Section } from './components/layout';
import { KifuTable } from './components/data';
import { Strength } from './components/strength';
import { api } from './api';
import type { ActivityView } from './api';
import type { Prefs } from './prefs';
import type { BookBrowse } from './BookScreen';
import type { NavId } from './components/ggs';
import type { Move } from './components/data';
import { LEVELS } from './state';
import { t, tErr } from './i18n';

/* Tab ids are stable English tokens, not the visible captions: the
 * caption changes with the language, the id must not. Dock takes
 * captions, so map both ways here. */
const TABS = ['record', 'strength', 'learn'] as const;
type TabId = (typeof TABS)[number];

const tabLabel = (id: TabId): string =>
  id === 'record' ? t('dock.tab.record')
    : id === 'strength' ? t('dock.tab.strength')
      : t('dock.tab.learning');

/** Unknown ids (a caller still on the old captions) fall back to the
 *  first tab rather than rendering an empty dock. */
const tabOf = (id: string): TabId =>
  (TABS as readonly string[]).includes(id) ? (id as TabId) : 'record';

/* Right dock for play/study, extracted from App (was 117 inline
 * lines). Its three tabs (record / strength / learning) only display
 * current state. Only the record tab avoids whole-dock scrolling: the
 * table pins its header and scrolls rows. */
export function PlayDock({
  g, book, cpu, prefs, tab, onTab, open, onNav, onBookTab,
  onPaste, onLoadFile, study, moves, ggfNames,
}: {
  g: ReturnType<typeof import('./state').useGame>;
  book: BookBrowse;
  cpu: ActivityView | null;
  prefs: Prefs;
  tab: string;
  onTab: (id: string) => void;
  open: boolean;
  onNav: (id: NavId) => void;
  onBookTab: (id: string) => void;
  onPaste: () => void;
  onLoadFile: () => void;
  study: boolean;
  /** Record-table rows; App builds them from its records. */
  moves: Move[];
  /** Builds the save filename (includes the played color); owned by App. */
  ggfNames: () => [string, string];
}) {
  const active = tabOf(tab);
  const labels = TABS.map(tabLabel);
  return (
    <Dock tabs={labels} active={tabLabel(active)}
          onTab={(label) => onTab(TABS[labels.indexOf(label)] ?? 'record')}
          open={open} scroll={active !== 'record'}>
      {active === 'record' && (
        <>
          <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
            <KifuTable moves={moves} current={g.view?.cursor} decimals={prefs.decimals}
                       onSelect={(n) => void g.jumpTo(n)} />
          </div>
          {/* Record I/O sits BELOW the column: on top it would lead
              the eye even when only reading, and rows grow downward, so
              hand-off actions belong at the end. */}
          <div style={{
            flex: 'none', display: 'flex', gap: 'var(--sp-2)',
            padding: 'var(--sp-2) var(--sp-3)', borderTop: '1px solid var(--border-weak)',
          }}>
            {/* Study hides this (the toolbar opens the same overlay);
                never two copies of one action per screen. Play shows
                the three buttons as designed. */}
            {!study && <Button title="⌘O" onClick={() => onPaste()}>{t('dock.record.paste')}</Button>}
            <Button onClick={() => void onLoadFile()}>{t('dock.record.load')}</Button>
            {/* .ggf saves colors, result and start position. Disabled
                with no moves — pressing would only bounce, and the
                table right above already says why. */}
            <Button title="⌘S" disabled={!moves.length}
                    onClick={() => void api.saveKifu(...ggfNames()).catch((e: unknown) => g.say(tErr(e)))}>
              {t('dock.record.save')}
            </Button>
          </div>
        </>
      )}
      {active === 'strength' && (
        // In study the same three become analysis depth; only the
        // section title changes.
        <Section title={study ? t('dock.strength.analysis_title') : t('dock.tab.strength')}>
          {/* Same Strength picker as the GGS settings — one learned
              interaction serves both. */}
          <Strength value={g.levels} onChange={(v) => {
            const i = LEVELS.findIndex((l) => l.depth === v.depth && l.solve === v.solve && l.band === v.band);
            if (i >= 0) { g.setLevel(i); return; }
            g.setCustom(v);
            g.setLevel('custom');
          }} />
          {/* Label and disabled-reason on one line, the picker full
              width below; side-by-side at 290px squeezes the options
              and orphans the hint. */}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
            <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-2)' }}>
              <span style={{ fontSize: 'var(--fs-5)' }}>{t('dock.book.label')}</span>
              {!g.hasBook && (
                <span style={{ fontSize: 'var(--fs-6)', color: 'var(--gold)' }}>
                  {t('dock.book.missing')}
                </span>
              )}
            </div>
            <Segmented fill value={g.useBook ? 'on' : 'off'} disabled={!g.hasBook}
                       onChange={(x) => g.setUseBook(x === 'on')}
                       options={[{ value: 'on', label: t('dock.book.use') },
                                 { value: 'off', label: t('dock.book.dont_use') }]} />
          </div>
        </Section>
      )}
      {active === 'learn' && (
        <>
          <Section title={t('dock.learn.writeback_title')}>
            <Toggle checked={g.learnOn} onChange={g.setLearnOn} label={t('dock.learn.import_toggle')} />
            <Note>{t('dock.learn.writeback_note')}</Note>
          </Section>
          {/* Framed only while running (the sole nesting exception is
              an active job). No pause button — the engine has no stop
              hook and yielding is automatic; say "yielding" instead of
              drawing a dead button. */}
          {/* Frame shown only while running, deliberately untitled per
              the design. No pause button (no engine hook; yielding is
              automatic). */}
          {cpu?.learn && (
            <div style={{
              border: '1px solid var(--border)', borderRadius: 'var(--r-2)',
              margin: '0 var(--sp-3)',
              padding: 'var(--sp-3)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
            }}>
              <div style={{ display: 'flex', alignItems: 'center', fontSize: 'var(--fs-5)' }}>
                <Busy>{t('dock.learn.importing')}</Busy>
                {/* Import advances per position; jittering digits would
                    shake the label too. */}
                <span style={{
                  marginLeft: 'auto', fontSize: 'var(--fs-6)', color: 'var(--sub)',
                  fontVariantNumeric: 'tabular-nums',
                }}>
                  {t('dock.learn.progress', {
                    done: cpu.learn[0].toLocaleString(),
                    total: cpu.learn[1].toLocaleString(),
                  })}
                </span>
              </div>
              <Progress value={cpu.learn[1] > 0 ? cpu.learn[0] / cpu.learn[1] : 0} />
              {/* Shown on the lower row only while yielding (as
                  designed); the engine offers no game-name hook. */}
              {cpu.learn_paused && (
                <div style={{ display: 'flex', alignItems: 'center', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                  <span>{t('dock.learn.yielding_note')}</span>
                  <span style={{ marginLeft: 'auto' }}>{t('dock.learn.yielding')}</span>
                </div>
              )}
            </div>
          )}
          {/* Details moved to the Book screen's second pane; a second
              copy here would compete with it. */}
          <Section title={t('dock.learn.book_title')}>
            <KeyValue big label={t('dock.learn.stored')} value={book.node?.size} />
            <KeyValue big label={t('dock.learn.learned')} value={book.node?.learned_size} />
            <Button onClick={() => { onNav('book'); onBookTab('log'); }}>{t('dock.learn.open_log')}</Button>
          </Section>
        </>
      )}
    </Dock>
  );
}
