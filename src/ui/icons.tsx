// The icon set, stroke based so the app has no icon-font dependency. The paths are
// icons.paths, which the desktop app reads as well: one name and one SVG path per line.
import raw from './icons.paths?raw';

const PATHS: Record<string, string> = {};
for (const line of raw.split('\n')) {
  if (!line || line.startsWith('#')) continue;
  const at = line.indexOf(' ');
  PATHS[line.slice(0, at)] = line.slice(at + 1);
}

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
