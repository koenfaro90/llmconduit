/** Recent live sessions and the complete durable, filtered session table. */
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { getConnection, queryKeys } from '../../api/connection';
import type { ActiveSessionsResponse, HistoryRequest, SessionDetailResponse, SessionNode, SessionRow } from '../../api/types';
import { flowFilterStore } from '../../store/flowFilterStore';
import { useAuth } from '../../store/hooks';
import { navigate } from '../../router/useHashRoute';
import { Panel } from '../../components/ui/Panel';
import { DataTable } from '../../components/ui/DataTable';
import { FacetSelect, type FacetSelection } from '../../components/ui/FacetSelect';
import { NumericRangeSelect } from '../../components/ui/NumericRangeSelect';
import { emptyFacet, hasFacet, matchesFacet, mergeFacetOptions, serializeFacets } from '../../components/ui/facetModel';
import { useTableColumns } from '../../components/ui/useTableColumns';
import type { DataTableColumn, DataTablePagination } from '../../components/ui/dataTableModel';
import { FlowDetail } from '../../components/FlowDetail/FlowDetail';
import { RequestTable } from '../../components/RequestTable/RequestTable';
import { identityOptions } from '../../components/RequestTable/requestsModel';
import type { RequestColumn } from '../../components/RequestTable/requestColumns';
import { fmtTokens } from '../../components/FlowTable/format';
import { cn } from '../../lib/cn';
import { kindBadge, rollupRequests, sessionLabel } from './sessionsModel';
import { activeSessionRows, activeStubHistoryRow, tokenTotals, type ActiveSessionRow } from './activeSessionsModel';

type SessionFilters = {
  q: string; facets: Record<'user' | 'key' | 'harness' | 'kind', FacetSelection>;
  firstSince: string; firstUntil: string; lastSince: string; lastUntil: string;
  minRequests: string; maxRequests: string; minChildren: string; maxChildren: string;
  minInput: string; maxInput: string; minOutput: string; maxOutput: string; minInFlight: string; maxInFlight: string;
};
const emptySessionFilters = (): SessionFilters => ({ q: '', facets: { user: emptyFacet(), key: emptyFacet(),
  harness: emptyFacet(), kind: emptyFacet() }, firstSince: '', firstUntil: '', lastSince: '', lastUntil: '',
  minRequests: '', maxRequests: '', minChildren: '', maxChildren: '', minInput: '', maxInput: '',
  minOutput: '', maxOutput: '', minInFlight: '', maxInFlight: '' });
type SessionSort = { by: string; descending: boolean };
const DEFAULT_SESSION_SORT: SessionSort = { by: 'last', descending: true };
const numeric = (value: string) => value.trim() === '' ? undefined : Number(value);
const dateValue = (value: string) => value ? new Date(value).getTime() : undefined;
const activityDate = (ms: number) => new Date(ms).toLocaleString();

const DASH = '—';
const SESSION_REQUEST_COLUMNS: readonly RequestColumn[] = ['number', 'id', 'start', 'status', 'model', 'lineage', 'ttft', 'duration', 'tokensIn', 'newIn', 'tokensOut'];
const ACTIVE_REQUEST_COLUMNS: readonly RequestColumn[] = ['number', 'id', 'start', 'status', 'model', 'tokensIn', 'cached', 'tokensOut'];

export function SessionsView() {
  const { client } = getConnection();
  const [expandedActiveId, setExpandedActiveId] = useState<string | null>(null);
  const [filters, setFilters] = useState<SessionFilters>(emptySessionFilters);
  const [sort, setSort] = useState<SessionSort>(DEFAULT_SESSION_SORT);
  const visibleColumns = useTableColumns('sessions', sessionColumns());
  const active = useQuery({
    queryKey: queryKeys.activeSessions,
    queryFn: (): Promise<ActiveSessionsResponse> => client.activeSessions(),
    refetchInterval: 30_000,
  });
  const isAdmin = useAuth((s) => s.user?.is_admin ?? s.authMode !== 'users');
  const users = useQuery({ queryKey: queryKeys.users, queryFn: () => client.listUsers(), enabled: isAdmin, retry: false });
  const keys = useQuery({ queryKey: queryKeys.keys('all'), queryFn: () => client.listKeys(isAdmin ? 'all' : undefined), retry: false });
  const authUsers = useQuery({ queryKey: ['auth', 'users', 'session-attribution'], queryFn: () => client.authUsers(), retry: false });
  const authKeys = useQuery({ queryKey: ['auth', 'keys', 'session-attribution'], queryFn: () => client.authApiKeys(), retry: false });
  const facets = useQuery({ queryKey: ['history', 'sessions', 'facets'], queryFn: () => client.historySessionFacets(), retry: false });
  const identities = useMemo(() => identityOptions(authUsers.data?.users, authKeys.data?.api_keys, users.data?.users, keys.data?.keys),
    [authUsers.data, authKeys.data, users.data, keys.data]);
  const userName = (id: string | null) => id == null ? null : identities.users.find((user) => user.id === id)?.name ?? null;
  const keyLabel = (id: string | null) => id == null ? null : identities.keys.find((key) => key.id === id)?.name ?? null;
  const activeRows = useMemo(() => activeSessionRows(active.data), [active.data]);
  const activeById = useMemo(() => new Map(activeRows.map((row) => [row.session.id, row])), [activeRows]);
  const nowMs = Date.now();
  const activeNodes = activeRows.filter((row) => row.session.last_seen_ms >= nowMs - 5 * 60_000 ||
    row.requests.some((request) => request.status === 'running')).map((row): SessionNode => {
    const totals = tokenTotals(row.aggregate, row.requests);
    return { ...row.session, child_count: row.child_count ?? activeRows.filter((child) => child.session.parent_id === row.session.id).length,
      input_tokens: totals.input, output_tokens: totals.output,
      in_flight: row.requests.filter((request) => request.status === 'running').length,
      user_name: userName(row.session.user_id), key_name: keyLabel(row.session.virtual_key_id) };
  });
  const shownActive = sortSessionNodes(activeNodes.filter((row) => matchesSessionFilters(row, filters)), sort);
  const userOptions = mergeFacetOptions(identities.users.map((user) => ({ value: user.id, label: user.name })),
    [...(facets.data?.user_ids ?? []), ...activeRows.map((row) => row.session.user_id).filter((id): id is string => !!id)]);
  const keyOptions = mergeFacetOptions(identities.keys.map((key) => ({ value: key.id, label: key.name })),
    [...(facets.data?.key_ids ?? []), ...activeRows.map((row) => row.session.virtual_key_id).filter((id): id is string => !!id)]);
  const harnessOptions = mergeFacetOptions([], [...(facets.data?.harnesses ?? []), ...activeRows.map((row) => row.session.harness)]);
  const kindOptions = mergeFacetOptions([], facets.data?.kinds ?? ['declared', 'inferred']);

  return <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-auto" data-testid="sessions-view">
    <div className="border-b border-line bg-panel px-3 py-2">
      <h1 className="text-sm font-semibold uppercase tracking-[0.18em] text-text">sessions</h1>
      <SessionFilterBar filters={filters} onChange={setFilters} onClear={() => setFilters(emptySessionFilters())}
        userOptions={userOptions} keyOptions={keyOptions} harnessOptions={harnessOptions} kindOptions={kindOptions}
        visibleIds={visibleColumns} />
    </div>
    <section className="max-h-[38vh] min-h-[150px] shrink-0 overflow-auto border-b border-line" data-testid="active-sessions-section">
      <h2 className="sticky top-0 z-10 bg-panel-raised px-3 py-2 text-xs font-medium uppercase tracking-[0.14em] text-text-muted">
        Active sessions · last 5 minutes · {shownActive.length}
      </h2>
      {active.isError && <Unavailable testId="sessions-unavailable" error={active.error} />}
      {active.data && shownActive.length === 0 && <p className="px-3 py-4 text-xs text-text-muted" data-testid="sessions-empty">No sessions active in the last 5 minutes.</p>}
      {shownActive.length > 0 && <SessionNodeTable id="active-sessions" nodes={shownActive} nowMs={nowMs}
        selectedId={expandedActiveId} onSelect={(id) => setExpandedActiveId(expandedActiveId === id ? null : id)}
        expandedId={expandedActiveId} onToggle={(node) => setExpandedActiveId(expandedActiveId === node.id ? null : node.id)}
        renderExpanded={(node) => { const row = activeById.get(node.id); return row ? <ActiveSessionExpansion row={row} /> : null; }}
        sort={sort} onSort={setSort} emptyMessage="No active sessions." />}
    </section>
    <section className="flex min-h-[330px] flex-1 flex-col" data-testid="all-sessions-section">
      <h2 className="bg-panel-raised px-3 py-2 text-xs font-medium uppercase tracking-[0.14em] text-text-muted">All sessions · complete history</h2>
      <DurableTreePanel filters={filters} sort={sort} onSort={setSort} userName={userName} keyLabel={keyLabel} />
    </section>
  </div>;
}

function ActiveSessionExpansion({ row }: { row: ActiveSessionRow }) {
  const session = row.session;
  return <div className="bg-panel px-3 pb-2" data-testid="active-session-requests">
    <div className="overflow-x-auto">
      <RequestTable rows={row.requests.map((stub) => activeStubHistoryRow(stub, session))}
        columns={ACTIVE_REQUEST_COLUMNS} rowTestId="active-request" tableId="active-requests"
        emptyMessage="No requests on the live ring (older than 15 minutes)." />
    </div>
    <button type="button" onClick={() => { flowFilterStore.getState().setSession(session.id); navigate('flows'); }}
      className="mt-1 rounded-md border border-line px-2 py-0.5 text-[10px] uppercase tracking-[0.12em] text-text-muted transition-colors hover:text-text"
      data-testid="active-session-show-in-flows">show in flows</button>
  </div>;
}

/** Durable session roots, paged by last activity so old sessions remain reachable. */
function DurableTreePanel({ filters, sort, onSort, userName, keyLabel }: {
  filters: SessionFilters; sort: SessionSort; onSort: (sort: SessionSort) => void;
  userName: (id: string | null) => string | null; keyLabel: (id: string | null) => string | null;
}) {
  const { client } = getConnection();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [expandedRootId, setExpandedRootId] = useState<string | null>(null);
  const [selectedRequest, setSelectedRequest] = useState<HistoryRequest | null>(null);
  const [page, setPage] = useState(1);
  useEffect(() => { setPage(1); }, [filters, sort]);
  const [requestCursors, setRequestCursors] = useState<Array<{ before_ms: number; before_id: string } | null>>([null]);
  const requestCursor = requestCursors.at(-1);
  const selectSession = (id: string) => {
    setSelectedId(id);
    setSelectedRequest(null);
    setRequestCursors([null]);
  };
  const roots = useQuery({
    queryKey: [...queryKeys.sessions, 'table', filters, sort, page],
    queryFn: () => client.historySessionTable({ q: filters.q || undefined,
      facets: serializeFacets(filters.facets),
      first_since_ms: dateValue(filters.firstSince), first_until_ms: dateValue(filters.firstUntil),
      last_since_ms: dateValue(filters.lastSince), last_until_ms: dateValue(filters.lastUntil),
      min_requests: numeric(filters.minRequests), max_requests: numeric(filters.maxRequests),
      min_children: numeric(filters.minChildren), max_children: numeric(filters.maxChildren),
      min_input_tokens: numeric(filters.minInput), max_input_tokens: numeric(filters.maxInput),
      min_output_tokens: numeric(filters.minOutput), max_output_tokens: numeric(filters.maxOutput),
      min_in_flight: numeric(filters.minInFlight), max_in_flight: numeric(filters.maxInFlight),
      sort_by: sort.by, descending: sort.descending,
      offset: (page - 1) * 100, limit: 100 }),
    refetchInterval: 30_000,
    enabled: !sessionRangeError(filters),
  });
  const detail = useQuery({
    queryKey: selectedId ? [...queryKeys.session(selectedId), requestCursor] : ['history', 'sessions', '__none__'],
    queryFn: () => client.historySession(selectedId as string, { limit: 100,
      before_ms: requestCursor?.before_ms, before_id: requestCursor?.before_id }),
    enabled: !!selectedId,
    refetchInterval: 30_000,
  });
  const expandedRoot = useQuery({
    queryKey: expandedRootId ? [...queryKeys.session(expandedRootId), 'children'] : ['history', 'sessions', '__no_root__'],
    queryFn: () => client.historySession(expandedRootId as string, { limit: 1 }),
    enabled: !!expandedRootId,
    refetchInterval: 30_000,
  });
  const rootRows = roots.data?.sessions ?? [];
  const nowMs = Date.now();
  const rootPagination: DataTablePagination = {
    page, pageSize: 100, total: roots.data?.total,
    hasNext: (page * 100) < (roots.data?.total ?? 0),
    onPrevious: () => { setPage(Math.max(1, page - 1)); setSelectedId(null); setExpandedRootId(null); setSelectedRequest(null); setRequestCursors([null]); },
    onNext: () => { setPage(page + 1); setSelectedId(null); setExpandedRootId(null); setSelectedRequest(null); setRequestCursors([null]); },
  };

  return (
    <div className="flex min-h-0 flex-1 border-t border-line" data-testid="sessions-tree">
      <div className={cn('flex min-h-0 min-w-0 flex-col', selectedId ? 'w-[42%] min-w-[400px] border-r border-line' : 'w-full')}>
        <div className="border-b border-line bg-panel-raised px-3 py-1.5 text-[10px] uppercase tracking-[0.14em] text-text-muted">
          all retained sessions · page {page}
        </div>
        <div className="min-h-0 flex-1 overflow-auto">
          {roots.isError && <Unavailable testId="sessions-tree-unavailable" error={roots.error} />}
          <SessionNodeTable id="session-roots" nodes={rootRows} nowMs={nowMs} selectedId={selectedId}
            emptyMessage="No sessions on this page." pagination={rootPagination}
            sort={sort} onSort={(next) => { onSort(next); setPage(1); }} userName={userName} keyLabel={keyLabel}
            onSelect={selectSession} expandedId={expandedRootId}
            onToggle={(node) => { setExpandedRootId(expandedRootId === node.id ? null : node.id); selectSession(node.id); }}
            expandedNodes={expandedRoot.data?.children}
            renderExpanded={() => <div className="border-b border-line/60 bg-panel-raised/40" data-testid="session-inline-children">
              {expandedRoot.isPending && <div className="px-4 py-2 text-xs text-text-muted">Loading sub-sessions…</div>}
              {expandedRoot.isError && <Unavailable testId="session-children-unavailable" error={expandedRoot.error} />}
              {expandedRoot.data?.children.length === 0 && <div className="px-4 py-2 text-xs text-text-muted">No direct sub-sessions.</div>}
            </div>} />
        </div>
      </div>
      {selectedId && <div className={cn('flex min-h-0 min-w-0 flex-1 flex-col', selectedRequest ? '[&>section]:!w-full' : 'overflow-auto p-3')}>
        {selectedRequest && <FlowDetail key={selectedRequest.id} apiCallId={selectedRequest.id}
          historyRequest={selectedRequest} onClose={() => setSelectedRequest(null)} />}
        {!selectedRequest && <>
        {selectedId && detail.isError && <Unavailable testId="session-detail-unavailable" error={detail.error} />}
        {selectedId && detail.data && <TreeDetailView data={detail.data} nowMs={nowMs} onSelect={selectSession} onRequestSelect={setSelectedRequest}
          requestPage={requestCursors.length}
          onPreviousRequests={() => { setRequestCursors(requestCursors.slice(0, -1)); setSelectedRequest(null); }}
          onNextRequests={() => { if (detail.data?.next_before_ms != null && detail.data.next_before_id) {
            setRequestCursors([...requestCursors, { before_ms: detail.data.next_before_ms, before_id: detail.data.next_before_id }]); setSelectedRequest(null);
          } }} />}
        </>}
      </div>}
    </div>
  );
}

function Unavailable({ testId, error }: { testId: string; error: unknown }) {
  const message = error instanceof Error ? error.message : String(error);
  const disabled = message.includes('503');
  return (
    <div className="px-3 py-6 text-center text-xs text-text-muted" data-testid={testId} data-reason={disabled ? 'disabled' : 'error'}>
      {disabled
        ? 'Durable history is disabled: configure control_plane.storage (sqlite or postgres) to see retained sessions.'
        : `Could not load sessions: ${message}`}
    </div>
  );
}

function HarnessBadge({ node }: { node: SessionRow }) {
  return (
    <span
      className="shrink-0 rounded-sm bg-accent/15 px-1 text-[9px] uppercase tracking-wide text-accent"
      data-testid="session-harness"
      title={node.harness_version ? `${node.harness} ${node.harness_version}` : node.harness}
    >
      {node.harness}
    </span>
  );
}

function KindBadge({ node }: { node: SessionRow }) {
  const badge = kindBadge(node);
  return (
    <span
      className={cn('shrink-0 rounded-sm px-1 text-[9px] uppercase tracking-wide', badge.inferred ? 'bg-status-cooling/15 text-status-cooling' : 'bg-line/40 text-text-muted')}
      data-testid="session-kind"
      data-kind={node.kind}
      title={badge.title}
    >
      {badge.label}
    </span>
  );
}

function sessionColumns(userName?: (id: string | null) => string | null, keyLabel?: (id: string | null) => string | null): DataTableColumn<SessionNode>[] {
  return [
    { id: 'number', label: 'session', width: '108px', required: true, sortable: true, render: (node) =>
      <span className="font-mono text-text" title={node.external_id ?? node.id}>{node.depth > 0 ? '↳ ' : ''}{sessionLabel(node)}</span> },
    { id: 'id', label: 'UUID', width: '300px', defaultHidden: true, render: (node) => <span className="font-mono text-text-muted" title={node.id}>{node.id}</span> },
    { id: 'user', label: 'user', width: '140px', sortable: true, render: (node) => <span className="truncate" title={node.user_id ?? undefined}>{node.user_name ?? userName?.(node.user_id) ?? DASH}</span> },
    { id: 'key', label: 'key', width: '140px', sortable: true, render: (node) => <span className="truncate" title={node.virtual_key_id ?? undefined}>{node.key_name ?? keyLabel?.(node.virtual_key_id) ?? DASH}</span> },
    { id: 'harness', label: 'harness', width: '110px', sortable: true, render: (node) => <HarnessBadge node={node} /> },
    { id: 'kind', label: 'kind', width: '85px', sortable: true, render: (node) => <KindBadge node={node} /> },
    { id: 'role', label: 'role', width: '90px', sortable: true, render: (node) => <span className="text-accent" data-testid="session-role" title={node.session_kind ?? 'Role not reported by harness'}>{node.session_kind ?? '—'}</span> },
    { id: 'first', label: 'first activity', width: '155px', sortable: true, render: (node) => <span className="tabular-nums text-text-muted">{activityDate(node.first_seen_ms)}</span> },
    { id: 'last', label: 'last activity', width: '155px', sortable: true, render: (node) => <span className="tabular-nums text-text-muted">{activityDate(node.last_seen_ms)}</span> },
    { id: 'requests', label: 'requests', width: '92px', sortable: true, align: 'right', render: (node) => <span data-testid="session-request-count" title="requests linked to this node (measured)">{node.request_count}</span> },
    { id: 'children', label: 'subs', width: '70px', sortable: true, align: 'right', render: (node) => <span data-testid="session-child-count" title="direct sub-sessions">{node.child_count < 0 ? DASH : node.child_count}</span> },
    { id: 'input', label: 'tokens IN', width: '95px', sortable: true, align: 'right', render: (node) => fmtTokens(node.input_tokens) },
    { id: 'output', label: 'tokens OUT', width: '95px', sortable: true, align: 'right', render: (node) => fmtTokens(node.output_tokens) },
    { id: 'in_flight', label: 'in flight', width: '86px', sortable: true, align: 'right', render: (node) => node.in_flight ?? DASH },
  ];
}

function SessionNodeTable({ id, nodes, selectedId, onSelect, pagination, emptyMessage, expandedId, onToggle, renderExpanded, expandedNodes, showHeader = true, sort, onSort, userName, keyLabel }: {
  id: string; nodes: readonly SessionNode[]; nowMs: number; selectedId: string | null;
  onSelect: (id: string) => void; pagination?: DataTablePagination; emptyMessage?: string;
  expandedId?: string | null; onToggle?: (node: SessionNode) => void;
  renderExpanded?: (node: SessionNode) => ReactNode; showHeader?: boolean;
  expandedNodes?: readonly SessionNode[];
  sort?: SessionSort; onSort?: (sort: SessionSort) => void;
  userName?: (id: string | null) => string | null; keyLabel?: (id: string | null) => string | null;
}) {
  return <DataTable id={id} columnStateKey="sessions" rows={nodes} rowKey={(node) => node.id} columns={sessionColumns(userName, keyLabel)}
    sort={sort && onSort ? { ...sort, onChange: (by, descending) => onSort({ by, descending }) } : undefined}
    selectedKey={selectedId} onRowClick={(node) => onSelect(node.id)} expandOnRowClick={!!renderExpanded} rowTestId="session-row"
    rowAttributes={(node) => ({ title: node.external_id ? `${node.harness} session ${node.external_id}` : `${node.harness} (no session id on the wire)` })}
    canExpand={renderExpanded ? () => true : undefined} expandedKeys={expandedId ? [expandedId] : []}
    onToggleExpanded={onToggle} renderExpanded={renderExpanded} pagination={pagination}
    expandedRows={expandedNodes ? () => expandedNodes : undefined}
    emptyMessage={emptyMessage} showHeader={showHeader} showColumnChooser={showHeader} />;
}

function TreeDetailView({
  data,
  nowMs,
  onSelect,
  onRequestSelect,
  requestPage,
  onPreviousRequests,
  onNextRequests,
}: {
  data: SessionDetailResponse;
  nowMs: number;
  onSelect: (id: string) => void;
  onRequestSelect: (request: HistoryRequest) => void;
  requestPage: number;
  onPreviousRequests: () => void;
  onNextRequests: () => void;
}) {
  const { session, ancestors, children, requests } = data;
  const rollup = useMemo(() => rollupRequests(requests), [requests]);
  const showInFlows = () => {
    flowFilterStore.getState().setSession(session.id);
    navigate('flows');
  };
  return (
    <div className="flex flex-col gap-3" data-testid="session-detail">
      {/* breadcrumb: root … parent › this */}
      <nav className="flex flex-wrap items-center gap-1 text-xs" aria-label="session ancestors" data-testid="session-breadcrumb">
        {[...ancestors].reverse().map((node) => (
          <span key={node.id} className="flex items-center gap-1">
            <button type="button" className="font-mono text-text-muted hover:text-text" onClick={() => onSelect(node.id)}>
              {sessionLabel(node)}
            </button>
            <span className="text-text-muted">›</span>
          </span>
        ))}
        <span className="font-mono text-text">{sessionLabel(session)}</span>
      </nav>

      <Panel raised className="flex flex-wrap items-center gap-x-4 gap-y-1 px-3 py-2">
        <HarnessBadge node={session} />
        <KindBadge node={session} />
        {session.session_kind && (
          <span className="text-xs text-text-muted" data-testid="session-session-kind" title="session kind declared by the harness">
            {session.session_kind}
          </span>
        )}
        {session.harness === 'oh-my-pi' && !session.session_kind && (
          <span className="text-xs text-text-muted" title="OMP did not send a role identifier for this request group">role unknown</span>
        )}
        <Stat testId="session-stat-requests" label="requests" value={String(session.request_count)} quality="measured" />
        <Stat testId="session-stat-children" label="sub-sessions" value={String(session.child_count)} quality="measured" />
        <Stat testId="session-stat-depth" label="depth" value={String(session.depth)} quality="measured" />
        <Stat testId="session-stat-busts" label={`cache busts / ${rollup.count}`} value={rollup.quality === 'unavailable' ? DASH : String(rollup.busts)} quality={rollup.quality} accent={rollup.busts > 0 ? 'text-status-cooling' : undefined} />
        <Stat testId="session-stat-tokens" label="in · out · cached" value={rollup.inputTokens == null ? DASH : `${fmtTokens(rollup.inputTokens)} · ${fmtTokens(rollup.outputTokens)} · ${fmtTokens(rollup.cachedTokens)}`} quality={rollup.inputTokens == null ? 'unavailable' : 'derived'} />
        <span className="text-xs text-text-muted" title={session.client_label ?? 'no client attribution'}>
          {session.client_label ?? DASH}
        </span>
        <button
          type="button"
          onClick={showInFlows}
          className="ml-auto rounded-md border border-line px-2.5 py-1 text-xs text-text-muted transition-colors hover:text-text"
          data-testid="session-show-in-flows"
        >
          show in flows
        </button>
      </Panel>

      {children.length > 0 && (
        <section data-testid="session-children">
          <h2 className="mb-1 text-[10px] uppercase tracking-[0.14em] text-text-muted">sub-sessions · {children.length}</h2>
          <div className="rounded-md border border-line">
            <SessionNodeTable id="session-children" nodes={children} nowMs={nowMs} selectedId={null} onSelect={onSelect} />
          </div>
        </section>
      )}

      <section data-testid="session-requests">
        <h2 className="mb-1 text-[10px] uppercase tracking-[0.14em] text-text-muted">
          requests · page {requestPage} · {requests.length}
        </h2>
        <div className="overflow-x-auto rounded-md border border-line" data-testid="session-request-table">
          <RequestTable rows={requests} selectedId={null} onSelect={onRequestSelect} columns={SESSION_REQUEST_COLUMNS}
            rowTestId="session-request" tableId="session-requests" emptyMessage="No requests recorded on this node yet."
            pagination={{ page: requestPage, pageSize: 100, hasNext: data.requests_truncated && data.next_before_ms != null && !!data.next_before_id,
              onPrevious: onPreviousRequests, onNext: onNextRequests, previousLabel: 'Newer', nextLabel: 'Older' }} />
        </div>
      </section>
    </div>
  );
}

function Stat({ testId, label, value, quality, accent }: { testId: string; label: string; value: string; quality: 'measured' | 'derived' | 'unavailable'; accent?: string }) {
  return (
    <span className="flex flex-col" data-testid={testId} data-quality={quality} title={`${label}: ${quality}`}>
      <span className="text-[10px] uppercase tracking-[0.14em] text-text-muted">{label}</span>
      <span className={cn('font-mono text-sm tabular-nums', quality === 'unavailable' ? 'text-text-muted' : accent ?? 'text-text')}>{value}</span>
    </span>
  );
}

function matchesSessionFilters(row: SessionNode, filters: SessionFilters): boolean {
  const q = filters.q.trim().toLowerCase();
  if (q && ![row.id, row.display_number, row.display_number == null ? null : `S-${row.display_number}`, row.external_id, row.user_name, row.key_name, row.harness, row.session_kind]
    .some((value) => String(value ?? '').toLowerCase().includes(q))) return false;
  if (!matchesFacet(filters.facets.user, [row.user_id]) || !matchesFacet(filters.facets.key, [row.virtual_key_id]) ||
      !matchesFacet(filters.facets.harness, [row.harness]) || !matchesFacet(filters.facets.kind, [row.kind])) return false;
  const checks: Array<[string, string, number | null | undefined]> = [
    [filters.minRequests, filters.maxRequests, row.request_count], [filters.minChildren, filters.maxChildren, row.child_count],
    [filters.minInput, filters.maxInput, row.input_tokens], [filters.minOutput, filters.maxOutput, row.output_tokens],
    [filters.minInFlight, filters.maxInFlight, row.in_flight],
  ];
  if (checks.some(([min, max, actual]) => (min !== '' && (actual == null || actual < Number(min))) ||
    (max !== '' && (actual == null || actual > Number(max))))) return false;
  const firstSince = dateValue(filters.firstSince); const firstUntil = dateValue(filters.firstUntil);
  const lastSince = dateValue(filters.lastSince); const lastUntil = dateValue(filters.lastUntil);
  return (firstSince == null || row.first_seen_ms >= firstSince) && (firstUntil == null || row.first_seen_ms < firstUntil) &&
    (lastSince == null || row.last_seen_ms >= lastSince) && (lastUntil == null || row.last_seen_ms < lastUntil);
}

function sessionRangeError(filters: SessionFilters): string | null {
  for (const [label, min, max] of [
    ['Requests', filters.minRequests, filters.maxRequests], ['Sub-sessions', filters.minChildren, filters.maxChildren],
    ['Tokens IN', filters.minInput, filters.maxInput], ['Tokens OUT', filters.minOutput, filters.maxOutput],
    ['In flight', filters.minInFlight, filters.maxInFlight],
  ]) if (min !== '' && max !== '' && Number(min) > Number(max)) return `${label}: lower bound exceeds upper bound.`;
  return null;
}

function sortSessionNodes(rows: SessionNode[], sort: SessionSort): SessionNode[] {
  const field = ({ number: 'display_number', user: 'user_name', key: 'key_name', harness: 'harness', kind: 'kind',
    role: 'session_kind', first: 'first_seen_ms', last: 'last_seen_ms', requests: 'request_count',
    children: 'child_count', input: 'input_tokens', output: 'output_tokens', in_flight: 'in_flight' } as Record<string, keyof SessionNode>)[sort.by] ?? 'last_seen_ms';
  return [...rows].sort((a, b) => {
    const x = a[field]; const y = b[field];
    const cmp = typeof x === 'number' && typeof y === 'number' ? x - y : String(x ?? '').localeCompare(String(y ?? ''));
    return (sort.descending ? -cmp : cmp) || b.id.localeCompare(a.id);
  });
}

function SessionFilterBar({ filters, onChange, onClear, userOptions, keyOptions, harnessOptions, kindOptions, visibleIds }: {
  filters: SessionFilters; onChange: (next: SessionFilters) => void; onClear: () => void;
  userOptions: Array<{ value: string; label: string }>;
  keyOptions: Array<{ value: string; label: string }>;
  harnessOptions: Array<{ value: string; label: string }>;
  kindOptions: Array<{ value: string; label: string }>;
  visibleIds: readonly string[];
}) {
  const update = (key: keyof SessionFilters, value: string) => onChange({ ...filters, [key]: value });
  const setFacet = (key: keyof SessionFilters['facets'], value: FacetSelection) =>
    onChange({ ...filters, facets: { ...filters.facets, [key]: value } });
  const range = (label: string, minKey: keyof SessionFilters, maxKey: keyof SessionFilters) =>
    <NumericRangeSelect label={label} min={filters[minKey] as string} max={filters[maxKey] as string}
      onChange={(min, max) => onChange({ ...filters, [minKey]: min, [maxKey]: max })} />;
  const control = 'rounded border border-line bg-panel-raised px-2 py-1 text-xs normal-case tracking-normal text-text';
  const dateField = (label: string, key: keyof SessionFilters) => <label className="flex flex-col gap-1 text-[10px] uppercase tracking-wider text-text-muted">
    {label}<input type="datetime-local" aria-label={label} value={filters[key] as string} onChange={(event) => update(key, event.target.value)} className={control} />
  </label>;
  const show = (id: string) => visibleIds.includes(id);
  const hiddenActive: string[] = (Object.keys(filters.facets) as Array<keyof SessionFilters['facets']>)
    .filter((name) => !show(name) && hasFacet(filters.facets[name]));
  const hiddenRanges: Array<[string, string, string, string]> = [
    ['requests', 'Requests', filters.minRequests, filters.maxRequests], ['children', 'Sub-sessions', filters.minChildren, filters.maxChildren],
    ['input', 'Tokens IN', filters.minInput, filters.maxInput], ['output', 'Tokens OUT', filters.minOutput, filters.maxOutput],
    ['in_flight', 'In flight', filters.minInFlight, filters.maxInFlight],
  ];
  for (const [name, label, min, max] of hiddenRanges) if (!show(name) && (min || max)) hiddenActive.push(label);
  if (!show('first') && (filters.firstSince || filters.firstUntil)) hiddenActive.push('First activity');
  if (!show('last') && (filters.lastSince || filters.lastUntil)) hiddenActive.push('Last activity');
  return <div className="mt-2 flex flex-wrap items-end gap-2" aria-label="Session filters">
    <label className="flex flex-col gap-1 text-[10px] uppercase tracking-wider text-text-muted">Search
      <input aria-label="Search sessions" value={filters.q} onChange={(event) => update('q', event.target.value)} className={`${control} w-40`} placeholder="Number, role, name…" />
    </label>
    {show('user') && <FacetSelect label="User" options={userOptions} value={filters.facets.user} onChange={(value) => setFacet('user', value)} />}
    {show('key') && <FacetSelect label="Key" options={keyOptions} value={filters.facets.key} onChange={(value) => setFacet('key', value)} />}
    {show('harness') && <FacetSelect label="Harness" options={harnessOptions} value={filters.facets.harness} onChange={(value) => setFacet('harness', value)} />}
    {show('kind') && <FacetSelect label="Kind" options={kindOptions} value={filters.facets.kind} onChange={(value) => setFacet('kind', value)} />}
    {show('first') && dateField('First from', 'firstSince')}{show('first') && dateField('First until', 'firstUntil')}
    {show('last') && dateField('Last from', 'lastSince')}{show('last') && dateField('Last until', 'lastUntil')}
    {show('requests') && range('Requests', 'minRequests', 'maxRequests')}{show('children') && range('Sub-sessions', 'minChildren', 'maxChildren')}
    {show('input') && range('Tokens IN', 'minInput', 'maxInput')}{show('output') && range('Tokens OUT', 'minOutput', 'maxOutput')}{show('in_flight') && range('In flight', 'minInFlight', 'maxInFlight')}
    {hiddenActive.length > 0 && <span className="text-xs text-status-cooling" data-testid="session-hidden-filters">Hidden filters: {hiddenActive.join(', ')}</span>}
    <button type="button" onClick={onClear} className="rounded border border-line px-3 py-1.5 text-xs text-text-muted">Clear</button>
    {sessionRangeError(filters) && <p className="w-full text-xs text-status-down" role="alert">{sessionRangeError(filters)}</p>}
  </div>;
}
