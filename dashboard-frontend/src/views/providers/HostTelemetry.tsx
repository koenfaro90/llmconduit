import type { MeshNode } from '../../api/types';
import { Panel } from '../../components/ui/Panel';

const DASH = '—';
function percent(n: number | null | undefined) { return n == null ? DASH : `${n.toFixed(1)}%`; }
function bytes(n: number | null | undefined) {
  if (n == null) return DASH;
  return `${(n / 1024 ** 3).toFixed(1)} GiB`;
}
export function HostTelemetry({ nodes, now = Date.now() }: { nodes: MeshNode[]; now?: number }) {
  return <Panel className="p-4" data-testid="host-telemetry">
    <h2 className="text-sm font-semibold">Fleet hosts</h2>
    <p className="mt-1 text-xs text-text-muted">Live system and container observations</p>
    {!nodes.length && <p className="mt-3 text-xs text-text-muted">No enrolled hosts.</p>}
    <div className="mt-3 grid gap-3 lg:grid-cols-2">{nodes.map(node => {
      const host = node.host_snapshot;
      const age = host ? (now - Date.parse(host.sampled_at)) / 1000 : NaN;
      const stale = !Number.isFinite(age) || age > 20 || age < -30 || !node.enabled;
      return <div key={node.endpoint_id} className="min-w-0 rounded border border-line p-3" data-quality={!host ? 'unavailable' : stale ? 'stale' : 'measured'}>
        <div className="text-sm font-semibold">{node.label ?? host?.hostname ?? node.endpoint_id}</div>
        {!host ? <p className="mt-2 text-xs text-text-muted">Host telemetry unavailable.</p> : <>
          <p className="mt-1 text-[10px] text-text-muted">{host.hostname} · {host.architecture} · {stale ? 'stale sample' : 'live'} · sampled {host.sampled_at}</p>
          <dl className="mt-2 grid grid-cols-2 gap-2 text-xs">
            <div><dt>CPU</dt><dd>{percent(host.cpu?.utilization_percent)} · {host.cpu?.logical_cores ?? DASH} cores</dd></div>
            <div><dt>Memory used / total</dt><dd>{bytes(host.memory?.used_bytes)} / {bytes(host.memory?.total_bytes)}</dd></div>
            <div><dt>Disk available / total</dt><dd>{bytes(host.disk?.available_bytes)} / {bytes(host.disk?.total_bytes)}</dd></div>
          </dl>
          <div className="mt-2 space-y-1 text-xs">{host.network.map(net => <p key={net.name}>{net.name} · RX {net.rx_bytes_per_second == null ? DASH : `${(net.rx_bytes_per_second / 1024 ** 2).toFixed(2)} MiB/s`} · TX {net.tx_bytes_per_second == null ? DASH : `${(net.tx_bytes_per_second / 1024 ** 2).toFixed(2)} MiB/s`}</p>)}</div>
          <div className="mt-2 space-y-1 text-xs">{host.gpus.map(gpu => <p key={gpu.uuid}>GPU {gpu.index} · {gpu.name} · {percent(gpu.utilization_percent)} · {gpu.unified_memory ? 'unified memory (host RAM)' : `VRAM ${bytes(gpu.memory_used_mib == null ? null : gpu.memory_used_mib * 1024 ** 2)} / ${bytes(gpu.memory_total_mib == null ? null : gpu.memory_total_mib * 1024 ** 2)}`} · {gpu.temperature_celsius == null ? DASH : `${gpu.temperature_celsius} °C`} · {gpu.power_watts == null ? DASH : `${gpu.power_watts} W`}</p>)}</div>
          <div className="mt-3 text-xs">{host.containers.map(container => <div key={container.id} className="mt-1 break-words">
            <span className="font-mono">{container.name}</span> · {container.status || container.state} · CPU {percent(container.cpu_percent)} · RAM {bytes(container.memory_bytes)}
            <div className="text-[10px] text-text-muted">{container.image}</div>
          </div>)}</div>
          {host.errors.length > 0 && <p className="mt-2 text-xs text-status-cooling">Unavailable collectors: {host.errors.join(', ')}</p>}
        </>}
      </div>;
    })}</div>
  </Panel>;
}
