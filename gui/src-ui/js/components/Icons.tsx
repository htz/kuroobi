
export type IconName =
  | 'play' | 'study' | 'ggs-play' | 'ggs-lobby' | 'ggs-users' | 'ggs-chat'
  | 'ggs-standby' | 'ggs-tourney' | 'ggs-console' | 'login' | 'cpu' | 'memory'
  | 'check' | 'alert' | 'back' | 'close' | 'panel'
  | 'book' | 'results' | 'prefs';

const PATHS: Record<IconName, React.ReactNode> = {
  play: <>
    <circle cx="8.8" cy="12" r="5.2" />
    <circle cx="16.4" cy="12" r="6.1" fill="currentColor" stroke="none" />
  </>,
  study: <>
    <circle cx="10.5" cy="10.5" r="6.5" />
    <path d="M15.4 15.4 L20.5 20.5" />
  </>,
  'ggs-play': <>
    <circle cx="4.9" cy="12" r="3.6" />
    <circle cx="19.1" cy="12" r="4.3" fill="currentColor" stroke="none" />
    <path d="M13.9 3.4 9.2 11.6h2.9L10.1 20.6l4.7-8.2h-2.9z"
          fill="currentColor" stroke="none" />
  </>,
  'ggs-lobby': <>
    <circle cx="12" cy="7.2" r="2.9" />
    <path d="M6.6 19.5c0-3 2.4-4.7 5.4-4.7s5.4 1.7 5.4 4.7" />
    <path d="M6.2 10.4a2.4 2.4 0 1 0 0-4.4M2.5 18c0-2.2 1.3-3.6 3.2-4" />
    <path d="M17.8 10.4a2.4 2.4 0 1 1 0-4.4M21.5 18c0-2.2-1.3-3.6-3.2-4" />
  </>,
  'ggs-users': <>
    <circle cx="12" cy="8" r="3.6" />
    <path d="M5 20c0-3.9 3.1-6 7-6s7 2.1 7 6" />
  </>,
  'ggs-chat': <>
    <path d="M20 15.5a2 2 0 0 1-2 2H8.5L4.5 21v-3.5a2 2 0 0 1-.5-1.5v-9a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2z" />
  </>,
  'ggs-standby': <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M12 6.8V12l3.6 2.1" />
  </>,
  'ggs-tourney': <>
    <path d="M7.5 4.5h9v5a4.5 4.5 0 0 1-9 0z" />
    <path d="M7.5 6.5H4.5a3 3 0 0 0 3 3.5M16.5 6.5h3a3 3 0 0 1-3 3.5M12 14v3.5M8.5 20h7" />
  </>,
  'ggs-console': <>
    <rect x="3" y="4.5" width="18" height="15" rx="2" />
    <path d="M7.5 10l2.6 2.5-2.6 2.5M13 15h3.8" />
  </>,
  login: <>
    <path d="M13.5 4h4.5a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-4.5" />
    <path d="M9.5 16l4-4-4-4M13.5 12H3.5" />
  </>,
  cpu: <>
    <rect x="5.5" y="5.5" width="13" height="13" rx="1.5" />
    <rect x="9.5" y="9.5" width="5" height="5" rx="0.5" />
    <path d="M9 5.5V2.5M15 5.5V2.5M9 21.5v-3M15 21.5v-3M5.5 9H2.5M5.5 15H2.5M21.5 9h-3M21.5 15h-3" />
  </>,
  memory: <>
    <path d="M12 3.2 21 8l-9 4.8L3 8z" />
    <path d="M3 12.6 12 17.4l9-4.8M3 17.2 12 22l9-4.8" />
  </>,
  check: <>
    <path d="M4.5 12.5 9.5 17.5 19.5 6.5" />
  </>,
  alert: <>
    <path d="M12 3.5 22 20.5H2z" />
    <path d="M12 10v4.5M12 17.6v.1" />
  </>,
  back: <>
    <path d="M14.5 5.5 8 12l6.5 6.5" />
  </>,
  close: <>
    <path d="M6.5 6.5 17.5 17.5M17.5 6.5 6.5 17.5" />
  </>,
  panel: <>
    <rect x="3.2" y="4.6" width="17.6" height="14.8" rx="2.4" />
    <path d="M14.6 4.6v14.8" />
  </>,
  book: <>
    <path d="M12 6.6C10.4 5.2 8.2 4.6 4.5 4.6v12.6c3.7 0 5.9.6 7.5 2 1.6-1.4 3.8-2 7.5-2V4.6c-3.7 0-5.9.6-7.5 2z" />
    <path d="M12 6.6v14" />
  </>,
  prefs: <>
    <path d="M4 7.5h5M14 7.5h6M4 16.5h9M18 16.5h2" />
    <circle cx="11.5" cy="7.5" r="2.4" />
    <circle cx="15.5" cy="16.5" r="2.4" />
  </>,
  results: <>
    <path d="M4.5 20.5V13M12 20.5V6.5M19.5 20.5v-5" />
    <path d="M2.8 20.5h18.4" />
  </>,
};

export function Icon({ name, size = 17 }: { name: IconName; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24"
         fill="none" stroke="currentColor" strokeWidth={1.8}
         strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {PATHS[name]}
    </svg>
  );
}

export function IconButton({ name, label, onClick, size = 17 }: {
  name: IconName;
  label: string;
  onClick: () => void;
  size?: number;
}) {
  return (
    <button type="button" className="k-press" title={label} aria-label={label}
      onClick={onClick}
      style={{
        width: 32, height: 32, flex: 'none', border: 0, borderRadius: 'var(--r-2)',
        background: 'transparent', color: 'var(--sub)',
        display: 'inline-grid', placeItems: 'center',
        opacity: 1,
      }}>
      <Icon name={name} size={size} />
    </button>
  );
}
