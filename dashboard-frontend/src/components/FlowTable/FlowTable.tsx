/**
 * Virtualized flow table (mitmweb-style), newest-on-top. Columns: timestamp, short api_call_id,
 * client (user-agent), endpoint, requested→served model, upstream target, status chip, tokens
 * in/out, cost, elapsed. Error rows are red; running rows pulse (dot only); failover rows are
 * tagged. Driven by `useFlowRows` (live WS store ∪ `/flows` query). Row click selects the flow.
 *
 * Virtualization (`@tanstack/react-virtual`, fixed row height) keeps 10k rows smooth: only the
 * visible window + overscan is in the DOM. CRITICAL (D10): rows carry NO layout/FLIP transition —
 * only the status dot animates — so scrolling never thrashes. The header is a sibling of the
 * scroll container (not virtualized) so it stays put.
 */
import type { ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { getConnection, queryKeys } from '../../api/connection';
import { useAuth } from '../../store/hooks';
import type { FlowSummary, ModelPrice } from '../../api/types';
import { useDashboard, useFlowFilter } from '../../store/hooks';
import { flowFilterStore } from '../../store/flowFilterStore';
import { cn } from '../../lib/cn';
import { DataTable } from '../ui/DataTable';
import { mergeFacetOptions } from '../ui/facetModel';
import type { DataTableColumn } from '../ui/dataTableModel';
import { StatusChip } from './StatusChip';
import { TokensCell } from './TokensCell';
import { CacheEconomics } from './CacheEconomics';
import { ContextPressure } from './ContextPressure';
import { fmtClock, fmtElapsed, fmtModelPair } from './format';
import { costDisplay, elapsedMs, flowCost, isFailover, statusClass } from './flowModel';
import { clientCell } from './clientAttribution';
import { ClientRollup } from './ClientRollup';
import { FilterBar } from './FilterBar';
import { useFlowRows } from './useFlowRows';
import { useCatalog } from './useCatalog';

const ROW_HEIGHT = 30;

/**
 * The CLIENT cell (gap 15): renders the flow's NON-SECRET `client_label` (a key-hash `key-<hex>`, a
 * configured caller-id, or a WEAK User-Agent fallback) with a source-strength marker. The weak UA
 * fallback is rendered VISIBLY weaker (dimmed/italic + a `ua` badge) so it never reads as a confirmed
 * identity; a strong key-hash / configured-id carries its source badge. An UNATTRIBUTED flow renders
 * `—` (don't-lie-with-zeros — never a fabricated id). A raw key never reaches here (gap 04 hashes it).
 */
function ClientCellView({
  flow,
  userName,
  keyLabel,
}: {
  flow: FlowSummary;
  userName: (id: string | null) => string | null;
  keyLabel: (id: string | null) => string | null;
}) {
  const cell = clientCell(flow);
  // The resolved attribution: a user display name when the flow carries a
  // `user_id` (admin users query; id-prefix fallback), else the key label or
  // the already-displayed key-hash/client label. Never a fabricated name.
  const user = userName(flow.user_id ?? null);
  const key = keyLabel(flow.virtual_key_id ?? null);
  return (
    <span
      className="flex min-w-0 items-center gap-1"
      data-testid="flow-client"
      data-quality={cell.quality}
      data-strength={cell.strength}
      data-attributed={cell.attributed ? 'true' : 'false'}
      title={cell.detail}
    >
      <span className={cn('truncate', cell.weak ? 'italic text-text-muted/70' : 'text-text-muted')}>
        {cell.label}
      </span>
      {user && (
        <span
          className="shrink-0 rounded-sm bg-accent/15 px-1 text-[9px] uppercase tracking-wide text-accent"
          data-testid="flow-user"
          title={`user ${flow.user_id}${flow.virtual_key_id ? ` · key ${flow.virtual_key_id}` : ''}`}
        >
          {user}
        </span>
      )}
      {key && !user && (
        <span
          className="shrink-0 rounded-sm bg-line/40 px-1 text-[9px] uppercase tracking-wide text-text-muted"
          data-testid="flow-key"
          title={`virtual key ${flow.virtual_key_id}`}
        >
          {key}
        </span>
      )}
      {cell.badge && (
        <span
          className={cn(
            'shrink-0 rounded-sm px-1 text-[9px] uppercase tracking-wide',
            // The WEAK UA fallback is visually distinct (cooling/amber) from a strong identity (neutral).
            cell.weak
              ? 'bg-status-cooling/15 text-status-cooling'
              : 'bg-line/40 text-text-muted',
          )}
          data-testid="flow-client-source"
          data-source={cell.source ?? undefined}
          title={cell.weak ? `weak ${cell.sourceLabel} fallback — not a confirmed identity` : `${cell.sourceLabel} (strong identity)`}
        >
          {cell.badge}
        </span>
      )}
    </span>
  );
}

// Keep the client column wide enough to distinguish common labels such as key hashes and user agents.
// The id cell also carries the cache-bust marker without adding another column.
const FLOW_COLUMNS: DataTableColumn<FlowSummary>[] = [
  { id: 'time', label: 'time', width: '88px' },
  { id: 'number', label: '#', width: '72px', required: true },
  { id: 'id', label: 'UUID', width: '300px', defaultHidden: true },
  { id: 'client', label: 'client', width: 'minmax(120px,0.9fr)' },
  { id: 'user', label: 'user', width: '110px', defaultHidden: true },
  { id: 'key', label: 'key', width: '110px', defaultHidden: true },
  { id: 'harness', label: 'harness', width: '88px' },
  { id: 'session', label: 'session', width: '110px', defaultHidden: true },
  { id: 'cacheBust', label: 'cache bust', width: '82px', defaultHidden: true },
  { id: 'endpoint', label: 'endpoint', width: 'minmax(100px,0.8fr)' },
  { id: 'model', label: 'model', width: 'minmax(150px,1.4fr)' },
  { id: 'upstream', label: 'upstream', width: '96px' },
  { id: 'status', label: 'status', width: '84px' },
  { id: 'tokens', label: 'tokens', width: '120px', align: 'right' },
  { id: 'cost', label: 'cost', width: '72px', align: 'right' },
  { id: 'elapsed', label: 'elapsed', width: '72px', align: 'right' },
];

/** The cache-bust marker's copy per divergence kind (only kinds that bust render a marker). */
function bustTitle(kind: string | null | undefined): string {
  switch (kind) {
    case 'instructions_changed': return 'cache bust — the system/instructions block changed';
    case 'tools_changed': return 'cache bust — the tool list changed';
    case 'history_rewritten': return 'cache bust — earlier conversation history changed';
    default: return 'cache bust — the request diverged inside its predecessor\'s prefix';
  }
}

/** The harness cell: the detected profile name (+ version in the title), `—` when undetected. */
function HarnessCellView({ flow }: { flow: FlowSummary }) {
  const known = !!flow.harness;
  return (
    <span
      className={cn('truncate', known ? 'text-text-muted' : 'text-text-muted/60')}
      data-testid="flow-harness"
      data-quality={known ? 'measured' : 'unavailable'}
      title={known ? `${flow.harness}${flow.harness_version ? ` ${flow.harness_version}` : ''}${flow.session_id ? ` · session ${flow.session_id}` : ''}` : 'harness not detected (persistence off or unknown client)'}
    >
      {known ? flow.harness : '—'}
    </span>
  );
}

export function FlowTable({
  selectedId,
  onSelect,
}: {
  selectedId: string | null;
  onSelect: (apiCallId: string) => void;
}) {
  const { client } = getConnection();
  const filters = useFlowFilter((s) => s.filters);
  const setFilters = flowFilterStore.getState().setFilters;
  const { rows, total, models, upstreams, clients, harnesses, sessions, userIds, keyIds } = useFlowRows(filters);
  // Gap 04+: resolve the flow's `user_id`/`virtual_key_id` to display names via
  // the users/keys queries (admin-gated; falls back to id prefixes / the
  // key-hash label) so the CLIENT column answers "which user, which key".
  const isAdmin = useAuth((s) => s.user?.is_admin ?? s.authMode !== 'users');
  const users = useQuery({ queryKey: queryKeys.users, queryFn: () => client.listUsers(), enabled: isAdmin, retry: false });
  const keys = useQuery({ queryKey: queryKeys.keys('all'), queryFn: () => client.listKeys(isAdmin ? 'all' : undefined), retry: false });
  const userByName = (id: string | null) => (id == null ? null : users.data?.users.find((u) => u.id === id)?.username ?? `${id.slice(0, 8)}…`);
  const keyByLabel = (id: string | null) => (id == null ? null : keys.data?.keys.find((k) => k.id === id)?.label ?? null);
  // Gap 09: the per-model context-window capacities (gap-06 nullable `context_limit`), for the
  // aggregate context-pressure stat. A `null`/absent window is UNKNOWN ⇒ that flow is excluded from
  // the pressure figures (never a fabricated 0%/100%).
  const contextLimits = useCatalog();
  // SEEK coherence (finding 6): while seeking, an OPEN row's elapsed must derive from the FROZEN
  // cut `at_ms` (the snapshot instant) rather than wall-clock `Date.now()`, which would tick the
  // frozen view forward. `seekAtMs` is null while LIVE → rows fall back to `Date.now()` per render.
  const seekAtMs = useDashboard((s) => s.seekAtMs);
  const priceTable = useDashboard((s) => s.priceTable);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <DataTable id="flows" columnStateKey="flows-numeric" rows={rows} rowKey={(flow) => flow.api_call_id} columns={FLOW_COLUMNS}
        virtualize={{ rowHeight: ROW_HEIGHT, scrollTestId: 'flow-table-scroll' }} rowTestId="flow-row"
        emptyMessage="No flows match the current filters."
        controlsClassName="!border-0 !p-0"
        filterBar={(visibleIds) => <FilterBar
        filters={filters}
        models={models}
        upstreams={upstreams}
        clients={clients}
        harnesses={harnesses}
        sessions={sessions}
        userOptions={mergeFacetOptions((users.data?.users ?? []).map((user) => ({ value: user.id, label: user.username })), userIds)}
        keyOptions={mergeFacetOptions((keys.data?.keys ?? []).map((key) => ({ value: key.id, label: key.label ?? key.id })), keyIds)}
        total={total}
        shown={rows.length}
        visibleIds={visibleIds}
        onChange={setFilters}
      />}
        renderRow={(flow, visible) => <FlowRow flow={flow} columns={visible}
          priceTable={priceTable} nowMs={seekAtMs ?? Date.now()}
          selected={flow.api_call_id === selectedId} onSelect={onSelect}
          userName={userByName} keyLabel={keyByLabel} />} />
      {/* Gap 09: the AGGREGATE context-pressure stat — peak context-window utilization + near/over
          counts across the SAME filtered rows. An always-visible stat under the table (outside the
          virtualized scroll container, so it does not affect row layout). */}
      <ContextPressure rows={rows} limits={contextLimits} />
      {/* Gap 08: the AGGREGATE cache-hit rate / "$ saved" by model, rolled up over the SAME filtered
          rows the table shows. A collapsed secondary surface under the table (never inside the
          virtualized scroll container, so it does not affect row layout). */}
      <CacheEconomics rows={rows} priceTable={priceTable} />
      {/* Gap 15: the AGGREGATE "by client" roll-up — cost / errors / latency per non-secret client
          (key-hash / configured-id / weak-UA), over the SAME filtered rows. A collapsed secondary
          surface under the table; its rows cross-link into the per-client filter. */}
      <ClientRollup rows={rows} />
    </div>
  );
}

function FlowRow({
  flow,
  columns,
  priceTable,
  nowMs,
  selected,
  onSelect,
  userName,
  keyLabel,
}: {
  flow: FlowSummary;
  columns: readonly DataTableColumn<FlowSummary>[];
  priceTable: Record<string, ModelPrice>;
  /** Reference instant for an OPEN row's elapsed: the frozen cut `at_ms` while seeking, else now. */
  nowMs: number;
  selected: boolean;
  onSelect: (id: string) => void;
  userName: (id: string | null) => string | null;
  keyLabel: (id: string | null) => string | null;
}) {
  const klass = statusClass(flow.status, flow.terminal_reason);
  const isError = klass === 'client-error' || klass === 'server-error';
  const failover = isFailover(flow);
  // Gap 07: derive the dollar STRING and the `estimated` flag TOGETHER from the cost + the per-flow
  // `cost_confidence`, so an `estimated` row is visibly labelled and an `unavailable` one renders
  // `—` (never a fabricated `$0.00`) — the same contract the StatsStrip $/min chip + FlowDetail use.
  const cost = costDisplay(flowCost(flow, priceTable), flow.cost_confidence);

  const cells: Record<string, ReactNode> = {
    time: <span className="tabular-nums text-text-muted">{fmtClock(flow.started_ms)}</span>,
    number: <span className="flex min-w-0 items-center gap-1" title={flow.api_call_id}>
      <span className="truncate font-mono text-text-muted">{flow.display_number == null ? '—' : `R-${flow.display_number}`}</span>
      {flow.cache_bust === true && <span className="shrink-0 rounded-sm bg-status-cooling/15 px-1 text-[9px] uppercase tracking-wide text-status-cooling" data-testid="flow-cache-bust" data-kind={flow.divergence_kind ?? undefined} title={bustTitle(flow.divergence_kind)}>bust</span>}
    </span>,
    id: <span className="truncate font-mono text-text-muted" title={flow.api_call_id}>{flow.api_call_id}</span>,
    client: <ClientCellView flow={flow} userName={userName} keyLabel={keyLabel} />,
    user: <span className="truncate text-text-muted" title={flow.user_id ?? undefined}>{userName(flow.user_id ?? null) ?? '—'}</span>,
    key: <span className="truncate text-text-muted" title={flow.virtual_key_id ?? undefined}>{keyLabel(flow.virtual_key_id ?? null) ?? '—'}</span>,
    harness: <HarnessCellView flow={flow} />,
    session: <span className="truncate font-mono text-text-muted" title={flow.session_id ?? undefined}>{flow.session_id?.slice(0, 12) ?? '—'}</span>,
    cacheBust: <span className="text-text-muted">{flow.cache_bust == null ? '—' : flow.cache_bust ? 'Yes' : 'No'}</span>,
    endpoint: <span className="truncate font-mono">{flow.uri || '—'}</span>,
    model: <span className="flex min-w-0 items-center gap-1.5">
      <span className="truncate">{fmtModelPair(flow.model_requested, flow.model_served)}</span>
      {failover && <span className="shrink-0 rounded-sm bg-status-cooling/15 px-1 text-[9px] uppercase text-status-cooling" data-testid="failover-tag" title="failover / re-routed">failover</span>}
    </span>,
    upstream: <span className="truncate text-text-muted">{flow.upstream_target ?? '—'}</span>,
    status: <StatusChip status={flow.status} terminalReason={flow.terminal_reason} />,
    tokens: <TokensCell flow={flow} priceTable={priceTable} />,
    cost: <span className="flex items-center justify-end gap-1 text-right tabular-nums text-meta">
      <span data-testid="flow-cost" data-confidence={cost.confidence}>{cost.value}</span>
      {cost.estimated && <span className="shrink-0 rounded-sm bg-status-cooling/15 px-1 text-[9px] uppercase tracking-wide text-status-cooling" data-testid="flow-cost-est" title="cost is an estimate — a billed token class has no configured rate">est</span>}
    </span>,
    elapsed: <span className="text-right tabular-nums text-text-muted">{fmtElapsed(elapsedMs(flow, nowMs))}</span>,
  };

  return (
    <button
      type="button"
      onClick={() => onSelect(flow.api_call_id)}
      data-api-call-id={flow.api_call_id}
      data-selected={selected || undefined}
      title={flow.api_call_id}
      style={{ display: 'grid', gridTemplateColumns: columns.map((column) => column.width ?? 'minmax(0,1fr)').join(' '), gap: '0.5rem', padding: '0 0.75rem' }}
      className={cn(
        'h-full w-full items-center border-b border-line/50 text-left text-xs',
        // No transition on layout properties — only background color, so virtualized rows
        // recycling positions never trigger a FLIP.
        'transition-colors',
        isError ? 'text-status-down' : 'text-text',
        selected ? 'bg-accent/12' : 'hover:bg-accent/[0.06]',
      )}
    >
      {columns.map((column) => <span key={column.id} className="min-w-0 overflow-hidden">{cells[column.id]}</span>)}
    </button>
  );
}
