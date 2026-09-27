import { useEffect, useMemo, useState, type KeyboardEvent } from 'react';
import { useQuery } from '@tanstack/react-query';
import { getConnection, queryKeys } from '../../api/connection';
import type { HistoryRequest } from '../../api/types';
import { dashboardStore } from '../../store/dashboardStore';
import { useAuth, useDashboard } from '../../store/hooks';
import { fmtElapsed, fmtTokens, fmtTokensPerSec } from '../FlowTable/format';
import { DataTable } from '../ui/DataTable';
import type { DataTableColumn, DataTablePagination } from '../ui/dataTableModel';
import { decodingRate, identityOptions, requestDisplay, requestIdentity, requestSignals, type RequestDisplay, type RequestSignals, type UsageSample } from './requestsModel';
import { divergenceLabel } from './lineageModel';
import { ALL_REQUEST_COLUMNS, DEFAULT_HIDDEN_REQUEST_COLUMNS, type RequestColumn } from './requestColumns';

const HEADERS: Record<RequestColumn, { label: string; width: string; title?: string; numeric?: boolean }> = {
  number: { label: '#', width: 'w-[80px]' },
  start: { label: 'Start', width: 'w-[165px]' },
  end: { label: 'End', width: 'w-[165px]' },
  id: { label: 'UUID', width: 'w-[300px]' },
  user: { label: 'User', width: 'w-[120px]' },
  key: { label: 'Key', width: 'w-[130px]' },
  harness: { label: 'Harness', width: 'w-[100px]' },
  session: { label: 'Session', width: 'w-[130px]' },
  status: { label: 'Status', width: 'w-[100px]' },
  ttft: { label: 'TTFT', width: 'w-[70px]', numeric: true },
  duration: { label: 'Total duration', width: 'w-[105px]', numeric: true },
  tokensIn: { label: 'Tokens IN', width: 'w-[80px]', numeric: true, title: 'Upstream-reported usage; may arrive only at completion' },
  cached: { label: 'Cached IN', width: 'w-[90px]', numeric: true, title: 'Reported cached input tokens; — when not reported' },
  newIn: { label: 'New IN', width: 'w-[105px]', numeric: true, title: 'Uncached input tokens = reported input minus reported cached input; — means cache usage was not reported.' },
  tokensOut: { label: 'Tokens OUT', width: 'w-[80px]', numeric: true, title: 'Upstream-reported usage; may arrive only at completion' },
  decodeSpeed: { label: 'Decode speed', width: 'w-[105px]', numeric: true, title: 'Rate from consecutive reported output-token counts while decoding.' },
  model: { label: 'Model · alias → served', width: '' },
  provider: { label: 'Provider', width: 'w-[120px]' },
  protocol: { label: 'Protocol', width: 'w-[110px]' },
  kind: { label: 'Kind', width: 'w-[100px]' },
  lineage: { label: 'Lineage', width: 'w-[125px]' },
};

function requestTableColumnDefs(columns: readonly RequestColumn[] = ALL_REQUEST_COLUMNS): DataTableColumn<HistoryRequest>[] {
  return columns.map((column) => ({
    id: column,
    label: HEADERS[column].label,
    width: `${Number(HEADERS[column].width.match(/\d+/)?.[0]) || 180}px`,
    align: HEADERS[column].numeric ? 'right' : 'left',
    title: HEADERS[column].title,
    required: column === 'number',
    defaultHidden: DEFAULT_HIDDEN_REQUEST_COLUMNS.includes(column),
  }));
}

function dateTime(ms: number | null): string {
  return ms == null ? '—' : new Date(ms).toLocaleString();
}

export function RequestTable({ rows, selectedId = null, onSelect, columns = ALL_REQUEST_COLUMNS, rowTestId = 'request-row', tableId = 'requests', emptyMessage = 'No requests.', pagination }: {
  rows: HistoryRequest[];
  selectedId?: string | null;
  onSelect?: (row: HistoryRequest) => void;
  columns?: readonly RequestColumn[];
  rowTestId?: string;
  tableId?: string;
  emptyMessage?: string;
  pagination?: DataTablePagination;
}) {
  const { client } = getConnection();
  const [nowMs, setNowMs] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);
  const connection = useDashboard((state) => state.connection);
  const flows = useDashboard((state) => state.flows);
  const monitor = useDashboard((state) => state.monitor);
  const liveFlows = connection === 'seeking' ? null : flows;
  const signalsByResponse = useMemo(() => connection === 'seeking' ? new Map<string, RequestSignals>() : requestSignals(monitor), [monitor, connection]);
  const [usageSamples, setUsageSamples] = useState<Map<string, UsageSample[]>>(new Map());
  useEffect(() => {
    let lastFlows = dashboardStore.getState().flows;
    const samples = new Map<string, UsageSample[]>();
    const record = (nextFlows: typeof lastFlows) => {
      if (nextFlows === lastFlows) return;
      lastFlows = nextFlows;
      let changed = false;
      const atMs = performance.now();
      for (const [id, flow] of nextFlows) {
        const completion = flow.status === 'open' ? flow.usage?.completion : null;
        if (completion == null || !Number.isFinite(completion) || completion < 0) continue;
        const previous = samples.get(id) ?? [];
        if (previous.at(-1)?.completion === completion) continue;
        const next = previous.length && completion < previous.at(-1)!.completion
          ? [] : previous.filter((sample) => atMs - sample.atMs <= 5_000);
        samples.set(id, [...next, { atMs, completion }]);
        changed = true;
      }
      for (const id of samples.keys()) {
        if (nextFlows.get(id)?.status !== 'open') {
          samples.delete(id);
          changed = true;
        }
      }
      if (changed) setUsageSamples(new Map(samples));
    };
    lastFlows = new Map();
    record(dashboardStore.getState().flows);
    return dashboardStore.subscribe((state) => record(state.flows));
  }, []);
  const isAdmin = useAuth((state) => state.user?.is_admin ?? true);
  const authUsers = useQuery({ queryKey: ['auth', 'users', 'request-attribution'], queryFn: () => client.authUsers(), retry: false });
  const authKeys = useQuery({ queryKey: ['auth', 'keys', 'request-attribution'], queryFn: () => client.authApiKeys(), retry: false });
  const legacyUsers = useQuery({ queryKey: queryKeys.users, queryFn: () => client.listUsers(), enabled: isAdmin, retry: false });
  const legacyKeys = useQuery({ queryKey: queryKeys.keys('all'), queryFn: () => client.listKeys(isAdmin ? 'all' : undefined), retry: false });
  const identities = useMemo(() => identityOptions(authUsers.data?.users, authKeys.data?.api_keys, legacyUsers.data?.users, legacyKeys.data?.keys),
    [authUsers.data, authKeys.data, legacyUsers.data, legacyKeys.data]);

  const tableColumns = requestTableColumnDefs(columns);
  return <DataTable id={tableId} columnStateKey={`${tableId}-numeric`} rows={rows} rowKey={(row) => row.id} columns={tableColumns}
    rowTestId={rowTestId} tableTestId="request-table" pagination={pagination} emptyMessage={emptyMessage}
    className="h-full" renderRow={(row, visible) => {
      const live = liveFlows?.get(row.id) ?? null;
      const signals = signalsByResponse.get(live?.response_id ?? row.response_id ?? '');
      const display = requestDisplay(row, live, nowMs, signals, decodingRate(usageSamples.get(row.id) ?? [], performance.now()));
      return <RequestTableRow key={row.id} row={row} display={display} columns={visible.map((column) => column.id as RequestColumn)}
        identity={requestIdentity(row, identities)} selected={selectedId === row.id} onSelect={onSelect ? () => onSelect(row) : undefined} rowTestId={rowTestId} />;
    }} />;
}

function RequestTableRow({ row, display, columns, identity, selected, onSelect, rowTestId }: {
  row: HistoryRequest;
  display: RequestDisplay;
  columns: readonly RequestColumn[];
  identity: { user: string; key: string };
  selected: boolean;
  onSelect?: () => void;
  rowTestId: string;
}) {
  function onKeyDown(event: KeyboardEvent<HTMLTableRowElement>) {
    if (onSelect && (event.key === 'Enter' || event.key === ' ')) {
      event.preventDefault();
      onSelect();
    }
  }
  return <tr className={`border-b border-line/60 ${onSelect ? 'cursor-pointer hover:bg-accent/[0.06]' : ''} ${selected ? 'bg-accent/[0.09]' : ''}`}
    onClick={onSelect} onKeyDown={onSelect ? onKeyDown : undefined} tabIndex={onSelect ? 0 : undefined}
    aria-label={onSelect ? `Open request ${row.id}` : undefined} aria-selected={onSelect ? selected : undefined}
    data-testid={rowTestId} data-status={display.status} data-cache-bust={row.cache_bust === true ? 'true' : 'false'}>
    {columns.map((column) => <RequestCell key={column} column={column} row={row} display={display} identity={identity} />)}
  </tr>;
}

function RequestCell({ column, row, display, identity }: {
  column: RequestColumn;
  row: HistoryRequest;
  display: RequestDisplay;
  identity: { user: string; key: string };
}) {
  switch (column) {
    case 'start': return <td className="px-2 py-2 tabular-nums text-text-muted" title={row.id}>{dateTime(row.created_at_ms)}</td>;
    case 'end': return <td className="px-2 py-2 tabular-nums text-text-muted">{dateTime(display.endMs)}</td>;
    case 'number': return <td className="px-2 py-2 font-mono text-text" title={row.id}>{row.display_number == null ? '—' : `R-${row.display_number}`}</td>;
    case 'id': return <td className="truncate px-2 py-2 font-mono text-text-muted" title={row.id}>{row.id}</td>;
    case 'user': return <td className="truncate px-2 py-2 text-text" title={identity.user === '—' ? 'User name unavailable' : identity.user}>{identity.user}</td>;
    case 'key': return <td className="truncate px-2 py-2 text-text" title={identity.key === '—' ? 'Key name unavailable' : identity.key}>{identity.key}</td>;
    case 'harness': return <td className="truncate px-2 py-2 text-text-muted" title={row.harness_version ? `${row.harness ?? ''} ${row.harness_version}` : undefined}>{row.harness ?? '—'}</td>;
    case 'session': return <td className="truncate px-2 py-2 font-mono text-text-muted" title={display.session}>{display.session}</td>;
    case 'status': return <td className={`px-2 py-2 ${display.status === 'failed' ? 'text-status-down' : ['queued', 'prefilling', 'decoding'].includes(display.status) ? 'text-accent' : 'text-text'}`}
      title={display.status === 'queued' ? 'Accepted; preparing, routing, or waiting for an upstream response' : display.status === 'prefilling' ? 'Upstream responded; waiting for the next output delta' : undefined}>{display.status}</td>;
    case 'ttft': return <td className="px-2 py-2 text-right tabular-nums text-text-muted">{fmtElapsed(display.ttftMs)}</td>;
    case 'duration': return <td className="px-2 py-2 text-right tabular-nums text-text-muted" title={['queued', 'prefilling', 'decoding'].includes(display.status) ? 'Elapsed so far' : undefined}>{fmtElapsed(display.durationMs)}</td>;
    case 'tokensIn': return <td className="px-2 py-2 text-right tabular-nums text-text-muted">{fmtTokens(display.inputTokens)}</td>;
    case 'cached': return <td className="px-2 py-2 text-right tabular-nums text-text-muted">{fmtTokens(row.cached_tokens)}</td>;
    case 'newIn': return <td className="px-2 py-2 text-right tabular-nums text-text" data-testid="request-new-input"
      data-quality={display.newInputTokens == null ? 'unavailable' : 'derived'}
      data-cache-bust={row.cache_bust === true ? 'true' : 'false'}
      title={row.cache_bust === true
        ? `Possible prefix-cache bust (${row.divergence_kind ?? 'changed prefix'}). New IN uses upstream cache usage, not this heuristic.`
        : 'Uncached input tokens from upstream-reported usage; — means cached input was not reported.'}>
      {fmtTokens(display.newInputTokens)}
      {(display.newInputPercent != null || row.cache_bust === true) && <> <span className={`text-[10px] ${row.cache_bust === true ? 'text-status-cooling' : 'text-text-muted'}`}>
        {display.newInputPercent == null ? '' : `${display.newInputPercent}%`}{row.cache_bust === true ? `${display.newInputPercent == null ? '' : ' · '}bust` : ''}
      </span></>}
    </td>;
    case 'tokensOut': return <td className="px-2 py-2 text-right tabular-nums text-text-muted">{fmtTokens(display.outputTokens)}</td>;
    case 'decodeSpeed': return <td className="px-2 py-2 text-right tabular-nums text-text" data-testid="request-decode-speed" data-quality={display.decodingTokensPerSec == null ? 'unavailable' : 'measured'}>{fmtTokensPerSec(display.decodingTokensPerSec)}</td>;
    case 'model': return <td className="truncate px-2 py-2 text-text" title={display.model}>{display.model}</td>;
    case 'provider': return <td className="truncate px-2 py-2 text-text-muted">{row.backend ?? '—'}</td>;
    case 'protocol': return <td className="truncate px-2 py-2 text-text-muted">{row.client_protocol}</td>;
    case 'kind': return <td className="truncate px-2 py-2 text-text-muted">{row.session_kind ?? '—'}</td>;
    case 'lineage': {
      const lineage = divergenceLabel(row.divergence_kind);
      return <td className="truncate px-2 py-2 text-text-muted" data-testid="session-request-lineage" data-kind={row.divergence_kind ?? undefined}
        title={`${lineage.title}${row.chain_parent_request_id ? ` · extends ${row.chain_parent_request_id}` : ''}`}>
        {lineage.bust ? 'bust · ' : ''}{lineage.label}
      </td>;
    }
  }
}
