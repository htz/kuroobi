// The ways a moment is written on screen. Times are epoch seconds; one shape per purpose, so
// the same moment reads the same on every screen.
import { t } from './i18n';

const p2 = (n: number) => String(n).padStart(2, '0');
const at = (secs: number) => new Date(secs * 1000);
const dayStart = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();

/** "21:15" */
export const clock = (secs: number): string => { const d = at(secs); return `${p2(d.getHours())}:${p2(d.getMinutes())}`; };

/** "10/26" */
export const day = (secs: number): string => { const d = at(secs); return `${d.getMonth() + 1}/${p2(d.getDate())}`; };

/** A list's date column: the time for today, the day before that. */
export const whenShort = (secs: number): string =>
  !secs ? '' : dayStart(at(secs)) === dayStart(new Date()) ? clock(secs) : day(secs);

/** "10/26 21:15", for a detail line. */
export const whenFull = (secs: number): string => (!secs ? '' : `${day(secs)} ${clock(secs)}`);

/** A conversation list: the time for today, "Yesterday", then the day. */
export const whenChat = (secs: number): string => {
  const ago = Math.round((dayStart(new Date()) - dayStart(at(secs))) / 86400000);
  return ago <= 0 ? clock(secs) : ago === 1 ? t('chat.yesterday') : day(secs);
};

const MONTHS = ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec'];

/**
 * A moment GGS writes as text ("27 Oct 2026 00:42:41", or "2026-10-27 00:42:41"), in epoch
 * seconds; 0 when it cannot be read. The time is taken as written, in this machine's zone.
 */
export const secsOfText = (text: string): number => {
  const hms = /(\d{1,2}):(\d{2})(?::(\d{2}))?/.exec(text);
  const [h, mi, s] = hms ? [Number(hms[1]), Number(hms[2]), Number(hms[3] ?? 0)] : [0, 0, 0];
  const iso = /(\d{4})[-/](\d{1,2})[-/](\d{1,2})/.exec(text);
  if (iso) return new Date(Number(iso[1]), Number(iso[2]) - 1, Number(iso[3]), h, mi, s).getTime() / 1000;
  const ggs = /(\d{1,2}) ([A-Za-z]{3}) (\d{4})/.exec(text);
  const mon = ggs ? MONTHS.indexOf(ggs[2].toLowerCase()) : -1;
  return mon >= 0 ? new Date(Number(ggs![3]), mon, Number(ggs![1]), h, mi, s).getTime() / 1000 : 0;
};
