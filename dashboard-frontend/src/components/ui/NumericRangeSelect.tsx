import { useEffect, useRef, useState } from 'react';

/** Optional inclusive bounds for nonnegative count/token filters. */
export function NumericRangeSelect({ label, min, max, onChange }: {
  label: string;
  min: string;
  max: string;
  onChange: (min: string, max: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') setOpen(false); };
    document.addEventListener('pointerdown', close);
    document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', close); document.removeEventListener('keydown', escape); };
  }, []);
  const invalid = min !== '' && max !== '' && Number(min) > Number(max);
  const summary = min && max ? `${label}: ${min}–${max}` : min ? `${label}: ≥ ${min}` : max ? `${label}: ≤ ${max}` : `Any ${label.toLowerCase()}`;
  return <div ref={root} className="relative flex flex-col gap-1 text-[10px] uppercase tracking-wider text-text-muted">
    <span>{label}</span>
    <button type="button" aria-label={label} aria-expanded={open} onClick={() => setOpen(!open)} title={summary}
      className="max-w-44 min-w-32 truncate rounded border border-line bg-panel-raised px-2 py-1.5 text-left text-xs normal-case tracking-normal text-text">
      {summary} ▾
    </button>
    {open && <div className="absolute left-0 top-full z-40 mt-1 w-52 rounded border border-line bg-panel p-2 text-xs normal-case shadow-xl" data-testid={`${label.toLowerCase().replaceAll(' ', '-')}-range`}>
      <label className="block text-text-muted">At least
        <input type="number" min="0" step="1" aria-label={`${label} at least`} value={min} onChange={(event) => onChange(event.target.value, max)}
          className="mt-1 w-full rounded border border-line bg-panel-raised px-2 py-1 text-text" />
      </label>
      <label className="mt-2 block text-text-muted">At most
        <input type="number" min="0" step="1" aria-label={`${label} at most`} value={max} onChange={(event) => onChange(min, event.target.value)}
          className="mt-1 w-full rounded border border-line bg-panel-raised px-2 py-1 text-text" />
      </label>
      {invalid && <p role="alert" className="mt-2 text-status-down">Lower bound exceeds upper bound.</p>}
      {(min || max) && <button type="button" onClick={() => onChange('', '')} className="mt-2 text-text-muted hover:text-text">Clear {label.toLowerCase()}</button>}
    </div>}
  </div>;
}
