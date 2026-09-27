import { useMemo, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { getConnection, queryKeys } from '../../api/connection';
import type { HistoryRequest } from '../../api/types';
import { FlowDetail } from '../../components/FlowDetail/FlowDetail';
import { RequestTable } from '../../components/RequestTable/RequestTable';
import { ALL_REQUEST_COLUMNS, DEFAULT_HIDDEN_REQUEST_COLUMNS } from '../../components/RequestTable/requestColumns';
import { useAuth } from '../../store/hooks';
import { identityOptions } from '../../components/RequestTable/requestsModel';
import { FacetSelect, type FacetSelection } from '../../components/ui/FacetSelect';
import { emptyFacet, mergeFacetOptions, serializeFacets } from '../../components/ui/facetModel';
import { hasFacet } from '../../components/ui/facetModel';
import { useTableColumns } from '../../components/ui/useTableColumns';

const PAGE_SIZE = 50;
const CLEAR_PHRASE = 'DELETE ALL REQUESTS AND SESSIONS';
type RequestFacetName = 'user' | 'key' | 'status' | 'model' | 'provider' | 'protocol' | 'harness' | 'kind' | 'session';
type Filters = {
  q: string; facets: Record<RequestFacetName, FacetSelection>; since: string; until: string;
};
type Cursor = { before_ms: number; before_id: string };
const emptyFilters = (): Filters => ({ q: '', facets: { user: emptyFacet(), key: emptyFacet(), status: emptyFacet(),
  model: emptyFacet(), provider: emptyFacet(), protocol: emptyFacet(), harness: emptyFacet(), kind: emptyFacet(), session: emptyFacet() }, since: '', until: '' });

function epochMs(value: string): number | undefined {
  const ms = value ? new Date(value).getTime() : NaN;
  return Number.isFinite(ms) ? ms : undefined;
}

export function RequestsView() {
  const { client, queryClient } = getConnection();
  const [filters, setFilters] = useState<Filters>(emptyFilters);
  const [cursors, setCursors] = useState<Array<Cursor | null>>([null]);
  const [selectedRequest, setSelectedRequest] = useState<HistoryRequest | null>(null);
  const [clearOpen, setClearOpen] = useState(false);
  const [clearPhrase, setClearPhrase] = useState('');
  const [clearNotice, setClearNotice] = useState<string | null>(null);
  // A dashboard-token login has no user even when user accounts also exist;
  // the server treats that token as an administrator.
  const isAdmin = useAuth((s) => s.user?.is_admin ?? true);
  const clearMutation = useMutation({
    mutationFn: () => client.clearHistory(clearPhrase),
    onSuccess: async ({ requests, sessions }) => {
      setSelectedRequest(null);
      setCursors([null]);
      setClearOpen(false);
      setClearPhrase('');
      setClearNotice(`Deleted ${requests} requests and ${sessions} sessions from durable history.`);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['history'] }),
        queryClient.invalidateQueries({ queryKey: queryKeys.activeSessions }),
      ]);
    },
  });
  const authUsers = useQuery({ queryKey: ['auth', 'users', 'request-attribution'], queryFn: () => client.authUsers(), retry: false });
  const authKeys = useQuery({ queryKey: ['auth', 'keys', 'request-attribution'], queryFn: () => client.authApiKeys(), retry: false });
  const legacyUsers = useQuery({ queryKey: queryKeys.users, queryFn: () => client.listUsers(), enabled: isAdmin, retry: false });
  const legacyKeys = useQuery({ queryKey: queryKeys.keys('all'), queryFn: () => client.listKeys(isAdmin ? 'all' : undefined), retry: false });
  const identities = useMemo(() => identityOptions(
    authUsers.data?.users, authKeys.data?.api_keys, legacyUsers.data?.users, legacyKeys.data?.keys,
  ), [authUsers.data, authKeys.data, legacyUsers.data, legacyKeys.data]);
  const visibleColumns = useTableColumns('requests-numeric', ALL_REQUEST_COLUMNS.map((id) => ({ id, label: id,
    required: id === 'number', defaultHidden: DEFAULT_HIDDEN_REQUEST_COLUMNS.includes(id) })));
  const show = (column: string) => visibleColumns.includes(column);
  const hiddenActive: string[] = (Object.keys(filters.facets) as RequestFacetName[]).filter((name) =>
    !show(name) && hasFacet(filters.facets[name]));
  if ((filters.since || filters.until) && !show('start')) hiddenActive.push('date');
  const facets = useQuery({ queryKey: ['history', 'requests', 'facets'], queryFn: () => client.historyRequestFacets(), retry: false, refetchInterval: 30_000 });
  const validationError = epochMs(filters.since) != null && epochMs(filters.until) != null &&
    epochMs(filters.since)! >= epochMs(filters.until)! ? 'From must be earlier than Until.' : null;
  const page = cursors.length;
  const cursor = cursors.at(-1);
  const listing = useQuery({
    queryKey: ['history', 'requests', filters, cursor],
    queryFn: () => client.historyRequests({
      limit: PAGE_SIZE,
      q: filters.q || undefined,
      facets: serializeFacets(filters.facets),
      since_ms: epochMs(filters.since),
      until_ms: epochMs(filters.until),
      before_ms: cursor?.before_ms,
      before_id: cursor?.before_id,
    }),
    retry: false,
    enabled: !validationError,
    // The SQL row is created at ingress and finalized asynchronously. Polling catches new
    // calls and terminal writes; the WS flow store fills in streaming fields between polls.
    refetchInterval: page === 1 ? 2_000 : 5_000,
  });
  const selected = listing.data?.requests.find((row) => row.id === selectedRequest?.id) ?? selectedRequest;

  function update(next: Filters) {
    setFilters(next);
    setCursors([null]);
    setSelectedRequest(null);
  }

  function clear() {
    update(emptyFilters());
  }

  const setFacet = (name: RequestFacetName, value: FacetSelection) =>
    update({ ...filters, facets: { ...filters.facets, [name]: value } });
  const userOptions = mergeFacetOptions(identities.users.map((user) => ({ value: user.id, label: user.name })), facets.data?.user_ids ?? []);
  const keyOptions = mergeFacetOptions(identities.keys.map((key) => ({ value: key.id,
    label: `${key.name}${key.userId ? ` · ${identities.users.find((user) => user.id === key.userId)?.name ?? 'unknown user'}` : ''}` })), facets.data?.key_ids ?? []);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden" data-testid="requests-view">
      <div className="border-b border-line bg-panel px-4 py-3">
        <div className="flex items-baseline gap-3">
          <h1 className="text-sm font-semibold uppercase tracking-[0.18em] text-text">requests</h1>
          <span className="text-xs text-text-muted">All retained requests · live while streaming · no time limit</span>
          {isAdmin && <button type="button" className="ml-auto rounded border border-status-down/60 px-2 py-1 text-xs text-status-down hover:bg-status-down/10"
            onClick={() => { setClearPhrase(''); setClearNotice(null); clearMutation.reset(); setClearOpen(true); }}>
            Clear history…
          </button>}
        </div>
        {clearNotice && <p className="mt-2 text-xs text-text-muted" role="status">{clearNotice}</p>}
        <div className="mt-3 flex flex-wrap items-end gap-2" aria-label="Request filters">
          <label className="flex min-w-[200px] flex-1 flex-col gap-1 text-[10px] uppercase tracking-wider text-text-muted">
            Search R-#, S-#, model, provider, harness, session
            <input aria-label="Search requests" value={filters.q} onChange={(event) => update({ ...filters, q: event.target.value })}
              placeholder="Search requests" maxLength={256} className="rounded border border-line bg-panel-raised px-2 py-1.5 text-xs normal-case tracking-normal text-text" />
          </label>
          {show('user') && <FacetSelect label="User" options={userOptions} value={filters.facets.user} onChange={(value) => setFacet('user', value)} />}
          {show('key') && <FacetSelect label="Key" options={keyOptions} value={filters.facets.key} onChange={(value) => setFacet('key', value)} />}
          {show('status') && <FacetSelect label="Status" options={facets.data?.statuses ?? []} value={filters.facets.status} onChange={(value) => setFacet('status', value)} />}
          {show('model') && <FacetSelect label="Model" options={facets.data?.models ?? []} value={filters.facets.model} onChange={(value) => setFacet('model', value)} />}
          {show('provider') && <FacetSelect label="Provider" options={facets.data?.providers ?? []} value={filters.facets.provider} onChange={(value) => setFacet('provider', value)} />}
          {show('protocol') && <FacetSelect label="Protocol" options={facets.data?.protocols ?? []} value={filters.facets.protocol} onChange={(value) => setFacet('protocol', value)} />}
          {show('harness') && <FacetSelect label="Harness" options={facets.data?.harnesses ?? []} value={filters.facets.harness} onChange={(value) => setFacet('harness', value)} />}
          {show('kind') && <FacetSelect label="Kind" options={facets.data?.kinds ?? []} value={filters.facets.kind} onChange={(value) => setFacet('kind', value)} />}
          {show('session') && <FacetSelect label="Session" options={facets.data?.sessions.map((session) => ({ value: session.id,
            label: session.display_number == null ? 'Unnumbered session' : `S-${session.display_number}` })) ?? []}
            value={filters.facets.session} onChange={(value) => setFacet('session', value)} />}
          {show('start') && <label className="flex flex-col gap-1 text-[10px] uppercase tracking-wider text-text-muted">
            From (optional)
            <input type="datetime-local" aria-label="From date" value={filters.since} onChange={(event) => update({ ...filters, since: event.target.value })}
              className="rounded border border-line bg-panel-raised px-2 py-1.5 text-xs normal-case text-text" />
          </label>}
          {show('start') && <label className="flex flex-col gap-1 text-[10px] uppercase tracking-wider text-text-muted">
            Until (optional)
            <input type="datetime-local" aria-label="Until date" value={filters.until} onChange={(event) => update({ ...filters, until: event.target.value })}
              className="rounded border border-line bg-panel-raised px-2 py-1.5 text-xs normal-case text-text" />
          </label>}
          {hiddenActive.length > 0 && <span className="text-xs text-status-cooling" data-testid="request-hidden-filters">Hidden filters: {hiddenActive.join(', ')}</span>}
          <div className="flex gap-2">
            <button type="button" onClick={clear} className="rounded border border-line px-3 py-1.5 text-xs text-text-muted hover:text-text">Clear</button>
          </div>
        </div>
        {validationError && <p className="mt-2 text-xs text-status-down" role="alert">{validationError}</p>}
      </div>
      <div className="flex min-h-0 min-w-0 flex-1">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <div className="min-h-0 flex-1 overflow-auto">
            {listing.isPending && <p className="p-4 text-xs text-text-muted">Loading requests…</p>}
            {listing.isError && <p className="p-4 text-xs text-status-down" role="alert">Could not load request history: {listing.error instanceof Error ? listing.error.message : String(listing.error)}</p>}
            {listing.data && <>
              <RequestTable rows={listing.data.requests} selectedId={selected?.id ?? null}
                onSelect={(row) => setSelectedRequest(selected?.id === row.id ? null : row)}
                emptyMessage="No requests match these filters."
                pagination={{ page, pageSize: PAGE_SIZE, hasNext: listing.data.has_more, busy: listing.isFetching,
                  onPrevious: () => { setCursors(cursors.slice(0, -1)); setSelectedRequest(null); },
                  onNext: () => { if (listing.data?.next_before_ms != null && listing.data.next_before_id) {
                    setCursors([...cursors, { before_ms: listing.data.next_before_ms, before_id: listing.data.next_before_id }]);
                    setSelectedRequest(null);
                  } } }} />
            </>}
          </div>
        </div>
        {selected && <FlowDetail key={selected.id} apiCallId={selected.id} historyRequest={selected} onClose={() => setSelectedRequest(null)} />}
      </div>
      {clearOpen && <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4" role="presentation">
        <div role="dialog" aria-modal="true" aria-label="Clear request and session history"
          className="max-h-[calc(100vh-2rem)] w-full max-w-md overflow-y-auto rounded-lg border border-status-down/50 bg-panel p-5 shadow-xl">
          <h2 className="text-sm font-semibold text-status-down">Delete all request and session history?</h2>
          <p className="mt-3 text-xs leading-5 text-text-muted">
            This permanently deletes stored inference requests, session trees, captured request bodies, and events.
            Users, keys, configuration, authentication sessions, and accounting/audit records stay.
            Requests already running can create new history after this clear.
          </p>
          <label className="mt-4 block text-xs text-text">Type <strong className="font-mono">{CLEAR_PHRASE}</strong> to confirm.
            <input autoFocus aria-label="Type confirmation phrase" value={clearPhrase} disabled={clearMutation.isPending}
              onChange={(event) => { setClearPhrase(event.target.value); clearMutation.reset(); }}
              className="mt-2 w-full rounded border border-line bg-panel-raised px-3 py-2 font-mono text-xs text-text" autoComplete="off" />
          </label>
          {clearPhrase === CLEAR_PHRASE && <p className="mt-3 text-xs text-text" role="status">
            Phrase matched. Nothing has been deleted. Click the button below to continue.
          </p>}
          {clearMutation.isError && <p className="mt-3 text-xs text-status-down" role="alert">
            Clear failed: {clearMutation.error instanceof Error ? clearMutation.error.message : String(clearMutation.error)}
          </p>}
          <div className="mt-5 flex justify-end gap-2">
            <button type="button" disabled={clearMutation.isPending} onClick={() => setClearOpen(false)}
              className="rounded border border-line px-3 py-2 text-xs text-text">Cancel</button>
            {(clearPhrase === CLEAR_PHRASE || clearMutation.isPending) && <button type="button"
              disabled={clearMutation.isPending} onClick={() => { if (clearPhrase === CLEAR_PHRASE) clearMutation.mutate(); }}
              className="rounded bg-status-down px-3 py-2 text-xs font-medium text-bg hover:bg-status-down/85 disabled:opacity-40">
              {clearMutation.isPending ? 'Clearing…' : 'Delete all history'}
            </button>}
          </div>
        </div>
      </div>}
    </div>
  );
}
