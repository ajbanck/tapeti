// Minimal inline icon set (16px, stroke based) so the app has no icon-font dependency.
const PATHS: Record<string, string> = {
  folder: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
  save: 'M5 3h11l4 4v13a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z M8 3v6h8V3 M8 21v-7h8v7',
  play: 'M7 4l13 8-13 8z',
  stop: 'M6 6h12v12H6z',
  plus: 'M12 5v14 M5 12h14',
  info: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M12 11v5 M12 8h.01',
  check: 'M4 12l5 5L20 6',
  sun: 'M12 4v2 M12 18v2 M4 12h2 M18 12h2 M6.3 6.3l1.4 1.4 M16.3 16.3l1.4 1.4 M6.3 17.7l1.4-1.4 M16.3 7.7l1.4-1.4 M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z',
  moon: 'M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z',
  monitor: 'M3 5h18v11H3z M8 20h8 M12 16v4',
  lock: 'M6 11h12v9H6z M8 11V8a4 4 0 0 1 8 0v3',
  unlock: 'M6 11h12v9H6z M8 11V8a4 4 0 0 1 7.5-2',
  chevron: 'M9 6l6 6-6 6',
  x: 'M6 6l12 12 M18 6L6 18',
  wave: 'M3 12h3l2-6 3 12 3-9 2 3h5',
  compare: 'M10 3v18 M14 3v18 M3 8h7 M14 8h7 M3 16h7 M14 16h7',
  hash: 'M5 9h14 M5 15h14 M10 3l-2 18 M16 3l-2 18',
  list: 'M4 6h2 M9 6h11 M4 12h2 M9 12h11 M4 18h2 M9 18h11',
  launch: 'M14 4h6v6 M20 4l-9 9 M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5',
  more: 'M4.5 12a.5.5 0 1 0 1 0 .5.5 0 1 0-1 0 M11.5 12a.5.5 0 1 0 1 0 .5.5 0 1 0-1 0 M18.5 12a.5.5 0 1 0 1 0 .5.5 0 1 0-1 0',
  cassette: 'M3 6h18a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1z M8 12a2 2 0 1 0 0 .01 M16 12a2 2 0 1 0 0 .01 M8 18l1-3h6l1 3',
};

export function Icon({ name, size = 16 }: { name: keyof typeof PATHS | string; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d={PATHS[name] ?? ''} />
    </svg>
  );
}

export function IconBtn({ name, title, onClick, disabled, active }: { name: string; title: string; onClick: () => void; disabled?: boolean; active?: boolean }) {
  return (
    <button class={'btn icon' + (active ? ' active' : '')} title={title} aria-label={title} disabled={disabled} onClick={onClick}>
      <Icon name={name} />
    </button>
  );
}
