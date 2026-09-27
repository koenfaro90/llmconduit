/**
 * ActivityView — who is using the gateway: requests, failures and tokens per user and per
 * key over a window, with a per-key request sparkline. Names come from the accounts API
 * (an admin sees everyone; a user sees their own keys); unattributed traffic (no key) is
 * listed as such rather than hidden.
 */
import { useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { getConnection, queryKeys } from '../../api/connection';
import { useAuth } from '../../store/hooks';
import { Panel } from '../../components/ui/Panel';
import { DataTable } from '../../components/ui/DataTable';
import type { DataTableColumn } from '../../components/ui/dataTableModel';
import { LineChart } from '../../viz/LineChart';
import { fmtTokens } from '../../components/FlowTable/format';
import { cn } from '../../lib/cn';
import { activityRows } from '../throughput/throughputModel';

const DASH = '—';
const WINDOWS: Array<{ label: string; ms: number; bucketSecs: number }> = [
  { label: '1h', ms: 60 * 60_000, bucketSecs: 300 },
  { label: '24h', ms: 24 * 60 * 60_000, bucketSecs: 3600 },
  { label: '7d', ms: 7 * 24 * 60 * 60_000, bucketSecs: 6 * 3600 },
];

export function ActivityView() {
  const { client } = getConnection();
  const isAdmin = useAuth((s) => s.user?.is_admin ?? s.authMode !== 'users');
  const [windowIdx, setWindowIdx] = useState(1);
  const win = WINDOWS[windowIdx]!;
  const sinceMs = Date.now() - win.ms;
  const activity = useQuery({
    queryKey: [...queryKeys.activity, win.label],
    queryFn: () => client.historyActivity({ since_ms: sinceMs, bucket_secs: win.bucketSecs }),
    refetchInterval: 30_000,
  });
  const users = useQuery({ queryKey: queryKeys.users, queryFn: () => client.listUsers(), enabled: isAdmin, retry: false });
  const keys = useQuery({ queryKey: queryKeys.keys('all'), queryFn: () => client.listKeys(isAdmin ? 'all' : undefined), retry: false });
  const rows = useMemo(() => activityRows(activity.data?.buckets ?? []), [activity.data]);
  const userName = (id: string | null) => (id == null ? 'unattributed' : users.data?.users.find((u) => u.id === id)?.username ?? id.slice(0, 8));
  const keyLabel = (id: string | null) => (id == null ? DASH : keys.data?.keys.find((k) => k.id === id)?.label ?? id.slice(0, 8));
  const totals = rows.reduce((a, r) => ({ requests: a.requests + r.requests, failed: a.failed + r.failed }), { requests: 0, failed: 0 });
  const columns: DataTableColumn<(typeof rows)[number]>[] = [
    { id: 'user', label: 'user', width: '16%', required: true, render: (row) => <span className={cn(row.user_id == null && 'italic text-text-muted')} data-testid="activity-user">{userName(row.user_id)}</span> },
    { id: 'key', label: 'key', width: '16%', render: (row) => <span className="font-mono text-text-muted" title={row.virtual_key_id ?? undefined}>{keyLabel(row.virtual_key_id)}</span> },
    { id: 'requests', label: 'reqs', width: '8%', align: 'right', render: (row) => row.requests },
    { id: 'failed', label: 'failed', width: '8%', align: 'right', render: (row) => <span className={cn(row.failed > 0 && 'text-status-down')}>{row.failed}</span> },
    { id: 'error', label: 'err %', width: '8%', align: 'right', render: (row) => row.error_pct == null ? DASH : row.error_pct.toFixed(1) },
    { id: 'input', label: 'in', width: '8%', align: 'right', render: (row) => fmtTokens(row.input_tokens) },
    { id: 'cached', label: 'cached', width: '8%', align: 'right', render: (row) => fmtTokens(row.cached_tokens) },
    { id: 'output', label: 'out', width: '8%', align: 'right', render: (row) => fmtTokens(row.output_tokens) },
    { id: 'trend', label: 'requests / bucket', width: '20%', render: (row) => <span className="block h-8"><LineChart height={32} width={160} series={[{ key: 'r', label: 'requests', points: row.series }]} formatY={() => ''} /></span> },
  ];

  return (
    <div className="min-h-0 min-w-0 flex-1 overflow-auto p-4" data-testid="activity-view">
      <div className="mb-3 flex flex-wrap items-baseline gap-2">
        <h1 className="text-sm font-semibold uppercase tracking-[0.18em] text-text">activity</h1>
        <span className="text-[10px] uppercase tracking-[0.14em] text-text-muted">requests · failures · tokens per user and key</span>
        <div className="ml-auto flex items-center gap-1" data-testid="activity-window">
          {WINDOWS.map((w, i) => (
            <button key={w.label} type="button" onClick={() => setWindowIdx(i)} className={cn('rounded-full border px-2.5 py-0.5 text-xs', i === windowIdx ? 'border-accent/40 bg-accent/15 text-accent' : 'border-line bg-panel text-text-muted')}>
              {w.label}
            </button>
          ))}
        </div>
      </div>
      {activity.isError && (
        <Panel className="mb-3 px-3 py-3 text-xs text-text-muted" data-testid="activity-unavailable">
          {String(activity.error).includes('503') ? 'Durable history is disabled: configure control_plane.storage (sqlite or postgres).' : `Could not load activity: ${String(activity.error)}`}
        </Panel>
      )}
      <Panel raised className="mb-3 flex flex-wrap gap-6 px-3 py-2" data-testid="activity-headline">
        <Stat testId="act-requests" label={`requests · ${win.label}`} value={totals.requests ? String(totals.requests) : DASH} quality={totals.requests ? 'measured' : 'unavailable'} />
        <Stat testId="act-failed" label="failed" value={totals.requests ? String(totals.failed) : DASH} quality={totals.requests ? 'measured' : 'unavailable'} />
        <Stat testId="act-principals" label="users · keys" value={rows.length ? `${new Set(rows.map((r) => r.user_id)).size} · ${new Set(rows.map((r) => r.virtual_key_id)).size}` : DASH} quality={rows.length ? 'measured' : 'unavailable'} />
      </Panel>
      {rows.length === 0 ? (
        <Panel className="px-3 py-3 text-xs text-text-muted" data-testid="activity-empty">No requests in this window.</Panel>
      ) : (
        <div className="rounded-md border border-line">
          <DataTable id="activity" rows={rows} rowKey={(row) => `${row.user_id}/${row.virtual_key_id}`}
            columns={columns} rowTestId="activity-row" rowAttributes={(row) => ({ 'data-user': row.user_id ?? 'none' })}
            clientPageSize={25} minWidth={900} />
        </div>
      )}
    </div>
  );
}

function Stat({ testId, label, value, quality }: { testId: string; label: string; value: string; quality: 'measured' | 'derived' | 'unavailable' }) {
  return (
    <span className="flex flex-col" data-testid={testId} data-quality={quality} title={`${label}: ${quality}`}>
      <span className="text-[10px] uppercase tracking-[0.14em] text-text-muted">{label}</span>
      <span className={cn('font-mono text-lg font-semibold tabular-nums', quality === 'unavailable' ? 'text-text-muted' : 'text-text')}>{value}</span>
    </span>
  );
}
