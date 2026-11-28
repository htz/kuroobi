export type Theme = 'auto' | 'light' | 'dark';

const KEY = 'kuroobi-theme';

export function savedTheme(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    return v === 'light' || v === 'dark' ? v : 'auto';
  } catch {
    return 'auto';
  }
}

/** tokens.css reads `<html data-theme>`; "auto" removes it so the OS decides. */
export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === 'auto') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', theme);
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // A private window keeps the choice for this page only.
  }
}
