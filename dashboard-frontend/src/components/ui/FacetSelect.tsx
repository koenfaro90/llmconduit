import { useEffect, useMemo, useRef, useState } from 'react';
import { toggleFacet, type FacetSelection } from './facetModel';

export type { FacetSelection } from './facetModel';
export type FacetOption = string | { value: string; label: string };

/** A reusable searchable facet with mutually exclusive include/exclude choices. */
export function FacetSelect({ label, options, value, onChange }: {
  label: string;
  options: readonly FacetOption[];
  value: FacetSelection;
  onChange: (next: FacetSelection) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') setOpen(false); };
    document.addEventListener('pointerdown', close);
    document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', close); document.removeEventListener('keydown', escape); };
  }, []);
  const choices = useMemo(() => {
    const byValue = new Map<string, { value: string; label: string }>();
    for (const option of options) {
      const item = typeof option === 'string' ? { value: option, label: option } : option;
      byValue.set(item.value, item);
    }
    for (const selected of [...value.include, ...value.exclude]) {
      if (!byValue.has(selected)) byValue.set(selected, { value: selected, label: selected });
    }
    return [...byValue.values()]
      .filter((option) => option.label.toLowerCase().includes(search.trim().toLowerCase()))
      .sort((a, b) => a.label.localeCompare(b.label));
  }, [options, search, value.include, value.exclude]);
  const count = value.include.length + value.exclude.length;
  const summary = count === 0 ? `All ${label.toLowerCase()}` :
    `${label}: ${value.include.length ? `+${value.include.length}` : ''}${value.include.length && value.exclude.length ? ' ' : ''}${value.exclude.length ? `−${value.exclude.length}` : ''}`;
  return <div ref={root} className="relative flex flex-col gap-1 text-[10px] uppercase tracking-wider text-text-muted">
    <span>{label}</span>
    <button type="button" aria-label={label} aria-expanded={open} onClick={() => { setOpen(!open); setSearch(''); }}
      className="max-w-44 min-w-32 truncate rounded border border-line bg-panel-raised px-2 py-1.5 text-left text-xs normal-case tracking-normal text-text"
      title={summary}>{summary} ▾</button>
    {open && <div className="absolute left-0 top-full z-40 mt-1 w-72 rounded border border-line bg-panel p-2 shadow-xl" data-testid={`${label.toLowerCase()}-facet`}>
      <input aria-label={`Search ${label.toLowerCase()}`} autoFocus value={search} onChange={(event) => setSearch(event.target.value)}
        className="mb-2 w-full rounded border border-line bg-panel-raised px-2 py-1 text-xs normal-case text-text" placeholder={`Search ${label.toLowerCase()}…`} />
      <div className="grid grid-cols-[3.5rem_minmax(0,1fr)_3.5rem] items-center border-b border-line pb-1 text-center text-[10px] uppercase tracking-wider text-text-muted">
        <span>Include</span><span>{label}</span><span>Exclude</span>
      </div>
      <div className="max-h-52 overflow-auto text-xs normal-case tracking-normal">
        {choices.length === 0 && <div className="px-2 py-2 text-text-muted">No matches</div>}
        {choices.slice(0, 200).map((option) => <div key={option.value} className="grid grid-cols-[3.5rem_minmax(0,1fr)_3.5rem] items-center py-1 text-text hover:bg-accent/10">
          <input type="checkbox" aria-label={`Include ${label}: ${option.label}`} checked={value.include.includes(option.value)}
            onChange={() => onChange(toggleFacet(value, option.value, 'include'))} className="mx-auto" />
          <span className="truncate text-center" title={option.label}>{option.label}</span>
          <input type="checkbox" aria-label={`Exclude ${label}: ${option.label}`} checked={value.exclude.includes(option.value)}
            onChange={() => onChange(toggleFacet(value, option.value, 'exclude'))} className="mx-auto" />
        </div>)}
      </div>
      {choices.length > 200 && <p className="mt-1 text-[10px] normal-case text-text-muted">Showing 200 of {choices.length}; search to narrow.</p>}
      {count > 0 && <button type="button" onClick={() => onChange({ include: [], exclude: [] })}
        className="mt-2 text-xs normal-case text-text-muted hover:text-text">Clear {label.toLowerCase()}</button>}
    </div>}
  </div>;
}
