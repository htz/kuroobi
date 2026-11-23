import { t } from './i18n';

/** Discs below the best book move a random pick may go; 0 always plays the best. */
export const BOOK_TOLS = [0, 0.5, 1, 2, 4];

export function bookTolOptions(): [string, string][] {
  return BOOK_TOLS.map((d) => [String(d), d === 0 ? t('book_tol.none') : t('book_tol.discs', { n: d })]);
}
