import { FLOW_STATUSES } from '../../api/types';
import { FacetSelect } from '../ui/FacetSelect';
import { emptyFacet, hasFacet } from '../ui/facetModel';
import { EMPTY_FILTERS, type FlowFacetName, type FlowFilters } from './filterTypes';

/** Categorical filters shared by live flows and every view that composes them. */
export function FilterBar({ filters, models, upstreams, clients, harnesses = [], sessions = [], userOptions = [], keyOptions = [],
  total, shown, visibleIds, onChange }: {
  filters: FlowFilters;
  models: string[];
  upstreams: string[];
  clients: string[];
  harnesses?: string[];
  sessions?: string[];
  userOptions?: Array<{ value: string; label: string }>;
  keyOptions?: Array<{ value: string; label: string }>;
  total: number;
  shown: number;
  visibleIds?: readonly string[];
  onChange: (next: FlowFilters) => void;
}) {
  const legacy = (key: FlowFacetName): string | null => key === 'cacheBust'
    ? filters.cacheBust === true ? 'true' : null
    : key === 'user' || key === 'key' ? null : filters[key];
  const selection = (key: FlowFacetName) => hasFacet(filters.facets[key]) ? filters.facets[key]
    : legacy(key) ? { include: [legacy(key)!], exclude: [] } : emptyFacet();
  const change = (key: FlowFacetName, value: { include: string[]; exclude: string[] }) => {
    const next: FlowFilters = { ...filters, facets: { ...filters.facets, [key]: value } };
    if (key === 'cacheBust') next.cacheBust = null;
    else if (key !== 'user' && key !== 'key') {
      // A dropdown edit replaces the one-value cross-link filter for this facet.
      Object.assign(next, { [key]: null });
    }
    onChange(next);
  };
  const active = (Object.keys(filters.facets) as FlowFacetName[]).some((key) => hasFacet(filters.facets[key]) || !!legacy(key));
  const columnFor: Record<FlowFacetName, string> = { status: 'status', model: 'model', upstream: 'upstream', client: 'client',
    user: 'user', key: 'key', harness: 'harness', session: 'session', cacheBust: 'cacheBust' };
  const show = (key: FlowFacetName) => !visibleIds || visibleIds.includes(columnFor[key]);
  const hiddenActive = (Object.keys(columnFor) as FlowFacetName[]).filter((key) => !show(key) &&
    (hasFacet(filters.facets[key]) || !!legacy(key)));
  const hiddenLabel: Partial<Record<FlowFacetName, string>> = { upstream: 'Provider', cacheBust: 'Cache bust' };

  return <div className="flex flex-wrap items-end gap-2 border-b border-line bg-panel px-3 py-2" data-testid="flow-filter-bar">
    {show('status') && <FacetSelect label="Status" options={FLOW_STATUSES} value={selection('status')} onChange={(value) => change('status', value)} />}
    {show('model') && <FacetSelect label="Model" options={models} value={selection('model')} onChange={(value) => change('model', value)} />}
    {show('upstream') && <FacetSelect label="Provider" options={upstreams} value={selection('upstream')} onChange={(value) => change('upstream', value)} />}
    {show('client') && <FacetSelect label="Client" options={clients} value={selection('client')} onChange={(value) => change('client', value)} />}
    {show('user') && <FacetSelect label="User" options={userOptions} value={selection('user')} onChange={(value) => change('user', value)} />}
    {show('key') && <FacetSelect label="Key" options={keyOptions} value={selection('key')} onChange={(value) => change('key', value)} />}
    {show('harness') && <FacetSelect label="Harness" options={harnesses} value={selection('harness')} onChange={(value) => change('harness', value)} />}
    {show('session') && <FacetSelect label="Session" options={sessions} value={selection('session')} onChange={(value) => change('session', value)} />}
    {show('cacheBust') && <FacetSelect label="Cache bust" options={[{ value: 'true', label: 'Yes' }, { value: 'false', label: 'No' }]}
      value={selection('cacheBust')} onChange={(value) => change('cacheBust', value)} />}
    {hiddenActive.length > 0 && <span className="text-xs text-status-cooling" data-testid="flow-hidden-filters">
      Hidden filters: {hiddenActive.map((key) => hiddenLabel[key] ?? key).join(', ')}
    </span>}
    {active && <button type="button" onClick={() => onChange(EMPTY_FILTERS)}
      className="rounded border border-line px-2 py-1.5 text-xs text-text-muted hover:text-text" data-testid="flow-filter-clear">Clear filters</button>}
    <span className="ml-auto tabular-nums text-xs text-text-muted" data-testid="flow-count">
      {shown === total ? `${total} flows` : `${shown} / ${total}`}
    </span>
  </div>;
}
