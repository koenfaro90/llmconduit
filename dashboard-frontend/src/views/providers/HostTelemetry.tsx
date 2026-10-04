import {StatusIcon} from '../../components/ui/StatusIcon';
import type { MeshNode } from '../../api/types';
import { DataTable } from '../../components/ui/DataTable';
import { Panel } from '../../components/ui/Panel';

const DASH = '—';
function pcieRate(n:number|null|undefined) {return n==null?DASH:`${(n/1000).toFixed(2)} MB/s`;}
function percent(n: number | null | undefined) { return n == null ? DASH : `${n.toFixed(1)}%`; }
function bytes(n: number | null | undefined) {
  if (n == null) return DASH;
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  const index = n > 0 ? Math.max(0, Math.min(4, Math.floor(Math.log(n) / Math.log(1024)))) : 0;
  return `${(n / 1024 ** index).toFixed(index === 0 && n >= 1 ? 0 : 1)} ${units[index]}`;
}
export function HostTelemetry({ nodes, now = Date.now() }: { nodes: MeshNode[]; now?: number }) {
  return <Panel className="p-4" data-testid="host-telemetry">
    <h2 className="text-sm font-semibold">Fleet hosts</h2>
    <p className="mt-1 text-xs text-text-muted">Live system and container observations</p>
    {!nodes.length && <p className="mt-3 text-xs text-text-muted">No enrolled hosts.</p>}
    <div className={`mt-3 grid gap-3 ${nodes.length>1?'lg:grid-cols-2':''}`}>{[...nodes].sort((a,b)=>(a.host_snapshot?.hostname??a.label??a.endpoint_id).localeCompare(b.host_snapshot?.hostname??b.label??b.endpoint_id,undefined,{numeric:true})).map(node => {
      const host = node.host_snapshot;
      const age = host ? (now - Date.parse(host.sampled_at)) / 1000 : NaN;
      const stale = !Number.isFinite(age) || age > 20 || age < -30 || !node.enabled;
      return <div key={node.endpoint_id} className="min-w-0 rounded border border-line p-3" data-quality={!host ? 'unavailable' : stale ? 'stale' : 'measured'}>
        <div className="text-sm font-semibold">{node.label ?? host?.hostname ?? node.endpoint_id}</div>
        {!host ? <p className="mt-2 text-xs text-text-muted">Host telemetry unavailable.</p> : <>
          <p className="mt-1 text-[10px] text-text-muted">{host.hostname} · {host.architecture} · {stale ? 'stale sample' : 'live'} · sampled {host.sampled_at}</p>
          {host.collection&&<p className="mt-1 text-[10px] text-text-muted">Local sampling: {host.collection.metrics?'snapshot':'host'} {host.collection.host_interval_ms} ms · GPU tick {host.collection.gpu_interval_ms} ms ({host.collection.gpu_backend||'pending'}) · containers {host.collection.container_interval_ms} ms. GPU read {host.collection.gpu_duration_ms?.toFixed(1)??DASH} ms; GPU sample {host.collection.gpu_sampled_at?new Date(host.collection.gpu_sampled_at).toLocaleTimeString():DASH}; containers {host.collection.containers_sampled_at?new Date(host.collection.containers_sampled_at).toLocaleTimeString():DASH}.</p>}
          {host.collection?.metrics&&<details className="mt-2 text-xs"><summary className="cursor-pointer text-text-muted">Metric sampling intervals</summary>
            <DataTable id="host-metric-sampling" interactive minWidth={320} showColumnChooser={false} mobileColumns={['metric','interval','sample']} rows={Object.entries(host.collection.metrics).sort(([a],[b])=>a.localeCompare(b)).map(([name,value])=>({name,...value}))} rowKey={row=>row.name} columns={[
              {id:'metric',label:'Metric',value:row=>row.name,render:row=>row.name.replaceAll('_',' ')},
              {id:'interval',label:'Interval',value:row=>row.interval_ms,render:row=>row.interval_ms===0?'Off':`${row.interval_ms} ms`},
              {id:'sample',label:'Last sample',value:row=>row.sampled_at??'',render:row=>row.sampled_at?new Date(row.sampled_at).toLocaleTimeString():DASH},
            ]}/>
          </details>}
          <dl className="mt-2 grid grid-cols-2 gap-2 text-xs">
            <div><dt>CPU</dt><dd>{percent(host.cpu?.utilization_percent)} · {host.cpu?.logical_cores ?? DASH} cores</dd></div>
            <div><dt>Memory used / total</dt><dd>{bytes(host.memory?.used_bytes)} / {bytes(host.memory?.total_bytes)}</dd></div>
            <div><dt>Disk available / total</dt><dd>{bytes(host.disk?.available_bytes)} / {bytes(host.disk?.total_bytes)}</dd></div>
          </dl>
          <section className="mt-3 min-w-0 space-y-2" aria-label="Host filesystems">
            <h3 className="text-xs font-semibold">Filesystems</h3>
            <DataTable id={`host-filesystems-${node.endpoint_id}`} interactive rows={host.filesystems??[]} rowKey={f=>f.mount_point} minWidth={720} mobileColumns={['mount','usage','available']} columns={[
              {id:'mount',label:'Mount',required:true,value:f=>f.mount_point,render:f=><span title={f.mount_points.join(', ')} className="break-all font-mono">{f.mount_point}{f.mount_points.length>1?` (+${f.mount_points.length-1} aliases)`:''}</span>},
              {id:'type',label:'Filesystem',value:f=>f.filesystem_type,render:f=>f.filesystem_type},
              {id:'device',label:'Device',value:f=>f.device,render:f=><span className="break-all">{f.device} ({f.device_id})</span>},
              {id:'usage',label:'Used / total',value:f=>f.total_bytes?f.used_bytes/f.total_bytes:null,render:f=>`${bytes(f.used_bytes)} / ${bytes(f.total_bytes)} (${percent(f.total_bytes?f.used_bytes/f.total_bytes*100:null)})`},
              {id:'available',label:'Available',value:f=>f.available_bytes,render:f=>bytes(f.available_bytes)},
              {id:'free',label:'Free (incl. reserved)',value:f=>f.free_bytes,render:f=>bytes(f.free_bytes)},
              {id:'inodes',label:'Inodes used / total',value:f=>f.inodes_total&&f.inodes_used!=null?f.inodes_used/f.inodes_total:null,render:f=>f.inodes_total&&f.inodes_used!=null?`${f.inodes_used.toLocaleString()} / ${f.inodes_total.toLocaleString()} (${percent(f.inodes_used/f.inodes_total*100)})`:DASH},
              {id:'access',label:'Access',value:f=>f.read_only?'Read only':'Read/write',render:f=><span title={f.read_only?'Host mount is read only':'Host mount is read/write'}><StatusIcon status={f.read_only?'read only':'read/write'}/><span className="ml-1">{f.read_only?'RO':'RW'}</span></span>},
            ]} emptyMessage="No filesystem sample available."/>
            <p className="text-[10px] text-text-muted">Host mounts, grouped by filesystem; aliases share capacity. Available excludes filesystem-reserved space. Access is the primary host mount’s mode. Network/pseudo filesystems and directory scans are excluded.</p>
          </section>
          <section className="mt-3 min-w-0 space-y-2" aria-label="Host disk I/O">
            <h3 className="text-xs font-semibold">Disk I/O</h3>
            <DataTable id={`host-block-devices-${node.endpoint_id}`} interactive rows={host.block_devices??[]} rowKey={d=>d.device_id} minWidth={760} mobileColumns={['name','read','write']} columns={[
              {id:'name',label:'Device',required:true,value:d=>d.name,render:d=><span className="font-mono">{d.name}</span>},
              {id:'layer',label:'Layer',value:d=>d.leaf_device?'Leaf':'Logical',render:d=>d.leaf_device?'Leaf':'Logical'},
              {id:'read',label:'Read / s',value:d=>d.read_bytes_per_second,render:d=>d.read_bytes_per_second==null?DASH:`${bytes(d.read_bytes_per_second)}/s`},
              {id:'write',label:'Write / s',value:d=>d.write_bytes_per_second,render:d=>d.write_bytes_per_second==null?DASH:`${bytes(d.write_bytes_per_second)}/s`},
              {id:'read-iops',label:'Read IOPS',value:d=>d.read_iops,render:d=>d.read_iops?.toFixed(1)??DASH},
              {id:'write-iops',label:'Write IOPS',value:d=>d.write_iops,render:d=>d.write_iops?.toFixed(1)??DASH},
              {id:'read-latency',label:'Read latency',value:d=>d.read_latency_ms,render:d=>d.read_latency_ms==null?DASH:`${d.read_latency_ms.toFixed(2)} ms`},
              {id:'write-latency',label:'Write latency',value:d=>d.write_latency_ms,render:d=>d.write_latency_ms==null?DASH:`${d.write_latency_ms.toFixed(2)} ms`},
              {id:'busy',label:'Busy',value:d=>d.busy_percent,render:d=>percent(d.busy_percent)},
              {id:'in-flight',label:'In flight',value:d=>d.in_flight,render:d=>d.in_flight},
              {id:'queue',label:'Avg. queue depth',value:d=>d.average_queue_depth,render:d=>d.average_queue_depth?.toFixed(2)??DASH},
            ]} emptyMessage="No disk I/O sample available."/>
            <p className="text-[10px] text-text-muted">Kernel counter deltas; initial/reset rates and idle latency remain —. Whole devices only; partitions are excluded. Leaf devices contribute to host totals; logical layers can represent the same I/O. Busy measures kernel-accounted time, not an NVMe performance ceiling.</p>
          </section>
          <div className="mt-2 space-y-1 text-xs">{host.network.map(net => <p key={net.name}>{net.name} · RX {net.rx_bytes_per_second == null ? DASH : `${bytes(net.rx_bytes_per_second)}/s`} · TX {net.tx_bytes_per_second == null ? DASH : `${bytes(net.tx_bytes_per_second)}/s`}</p>)}</div>
          <div className="mt-2 space-y-1 text-xs">{host.gpus.map(gpu => <p key={gpu.uuid}>GPU {gpu.index} · {gpu.name} · {percent(gpu.utilization_percent)} · {gpu.unified_memory ? 'unified memory (host RAM)' : `VRAM ${bytes(gpu.memory_used_mib == null ? null : gpu.memory_used_mib * 1024 ** 2)} / ${bytes(gpu.memory_total_mib == null ? null : gpu.memory_total_mib * 1024 ** 2)}`} · {gpu.temperature_celsius == null ? DASH : `${gpu.temperature_celsius} °C`} · {gpu.power_watts == null ? DASH : `${gpu.power_watts} W`} · Fan {percent(gpu.fan_speed_percent)} · SM {gpu.clock_mhz == null ? DASH : `${gpu.clock_mhz} MHz`} · memory clock {gpu.memory_clock_mhz == null ? DASH : `${gpu.memory_clock_mhz} MHz`}<span className="block text-text-muted">PCIe RX {pcieRate(gpu.pcie_rx_kb_per_second)} · TX {pcieRate(gpu.pcie_tx_kb_per_second)} · link Gen{gpu.pcie_link_generation??DASH} ×{gpu.pcie_link_width??DASH} (max Gen{gpu.pcie_max_link_generation??DASH} ×{gpu.pcie_max_link_width??DASH}) · replays {gpu.pcie_replay_count??DASH} · correctable errors {gpu.pcie_correctable_error_count??DASH} · recoveries {gpu.pcie_recovery_count??DASH}</span></p>)}</div>
          <p className="mt-1 text-[10px] text-text-muted">Fan is the driver’s intended operating percentage; unavailable sensors remain —. PCIe rates are GPU totals over the driver’s short measurement window. Link recoveries include normal power management; unsupported fields remain unavailable.</p>
          <div className="mt-3 text-xs">{host.containers.map(container => <div key={container.id} className="mt-1 break-words">
            <span className="font-mono">{container.name}</span> · {container.status || container.state} · CPU {percent(container.cpu_percent)} · RAM {bytes(container.memory_bytes)} · PIDs {container.pids??DASH}
            <div className="text-[10px] text-text-muted">{container.image}</div>
          </div>)}</div>
          {host.errors.length > 0 && <p className="mt-2 text-xs text-status-cooling">Unavailable collectors: {host.errors.join(', ')}</p>}
        </>}
      </div>;
    })}</div>
  </Panel>;
}
