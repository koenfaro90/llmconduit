import { useEffect, useMemo, useState } from 'react';
import type { MeshNode } from '../../api/types';
import { DataTable } from '../../components/ui/DataTable';
import { Panel } from '../../components/ui/Panel';
import { StatChip } from '../../components/ui/StatChip';
import { LineChart, ChartLegend, type ChartSeries } from '../../viz/LineChart';
import { HostTelemetry } from '../providers/HostTelemetry';
import { deploymentRows } from './managementModel';
import { useManagement } from './useManagement';
type Sample = {time:number; cpu:number|null; memory:number|null; gpu:number|null; rx:number|null; tx:number|null};
const percent = (n:number|null|undefined) => n == null ? '—' : `${n.toFixed(1)}%`;
const gib = (n:number|null|undefined) => n == null ? '—' : `${(n / 1024 ** 3).toFixed(1)} GiB`;
function live(node:MeshNode) {const age=Date.now()-Date.parse(node.host_snapshot?.sampled_at ?? '');return node.enabled && Number.isFinite(age) && age >= -30_000 && age <= 20_000;}
function networkRate(node:MeshNode,key:'rx_bytes_per_second'|'tx_bytes_per_second') {const nets=node.host_snapshot?.network.filter(n => n.name !== 'lo' && n[key] != null);return nets?.length ? nets.reduce((sum,n)=>sum+n[key]!,0) / 1024 ** 2 : null;}
export function HostsView() {
  const {nodes,state,error,loading}=useManagement();const [history,setHistory]=useState<Record<string,Sample[]>>({});const [query,setQuery]=useState('');
  useEffect(()=> {setHistory(old=>{
    const next:Record<string,Sample[]>={};
    for(const node of nodes.slice(0,64)) {const host=node.host_snapshot;const samples=old[node.endpoint_id] ?? [];if(!host){next[node.endpoint_id]=samples;continue;}const time=Date.parse(host.sampled_at);if(!Number.isFinite(time)||samples.at(-1)?.time===time){next[node.endpoint_id]=samples;continue;}
      const measured=live(node);const gpus=host.gpus.filter(g=>g.utilization_percent != null);
      const sample:Sample={time,cpu:measured?host.cpu?.utilization_percent ?? null:null,memory:measured&&host.memory?.total_bytes ? host.memory.used_bytes / host.memory.total_bytes * 100:null,gpu:measured&&gpus.length?gpus.reduce((sum,g)=>sum+g.utilization_percent!,0)/gpus.length:null,rx:measured?networkRate(node,'rx_bytes_per_second'):null,tx:measured?networkRate(node,'tx_bytes_per_second'):null};
      next[node.endpoint_id]=[...samples.filter(s=>s.time<time),sample].slice(-360);
    }return next;
  });},[nodes]);
  const deployments=useMemo(()=>deploymentRows(state,nodes),[state,nodes]);
  const filtered=nodes.filter(n=>`${n.host_snapshot?.hostname ?? ''} ${n.label ?? ''}`.toLowerCase().includes(query.toLowerCase())).sort((a,b)=>(a.host_snapshot?.hostname ?? a.label ?? a.endpoint_id).localeCompare(b.host_snapshot?.hostname ?? b.label ?? b.endpoint_id));
  return <div className="min-w-0 flex-1 overflow-auto p-4 space-y-4" data-testid="hosts-view">
    <header className="flex flex-wrap justify-between gap-3"><div><h1 className="text-lg font-semibold">Hosts</h1><p className="text-xs text-text-muted">Fleet system observations · 5-second refresh · charts collect up to 30 minutes while this page is open</p></div><input aria-label="Search hosts" placeholder="Find a host…" value={query} onChange={e=>setQuery(e.target.value)} className="rounded border border-line bg-panel p-2 text-xs" /></header>
    <Panel className="flex flex-wrap"><StatChip label="Hosts" value={nodes.length} /><StatChip label="Reporting" value={nodes.filter(live).length} accent="healthy" /><StatChip label="Inference containers ready" value={deployments.filter(d=>d.deployment.observed?.status?.phase==='ready').length} accent="accent" /><StatChip label="Stale / unavailable" value={nodes.filter(n=>!live(n)).length} accent="cooling" /></Panel>
    {error && <p role="alert">{String(error)}</p>}{loading && <p>Loading host observations…</p>}
    <div className="grid gap-4 xl:grid-cols-2">{filtered.map(node=>{const host=node.host_snapshot;const samples=history[node.endpoint_id] ?? [];const assigned=deployments.filter(d=>d.deployment.endpoint_id===node.endpoint_id&&d.deployment.desired_state==='ready');const utilizationSeries: ChartSeries[] = (['cpu','memory','gpu'] as const).map(k=>({key:k,label:k==='memory'?'RAM %':`${k.toUpperCase()} %`,points:samples.map(s=>[s.time,s[k]])}));const networkSeries:ChartSeries[] = (['rx','tx'] as const).map(k=>({key:k,label:`Network ${k.toUpperCase()} MiB/s`,points:samples.map(s=>[s.time,s[k]])}));return <Panel key={node.endpoint_id} className="p-4 space-y-3" data-quality={!host?'unavailable':live(node)?'measured':'stale'}>
      <div className="flex items-start justify-between gap-2"><div><h2 className="font-semibold">{host?.hostname ?? node.label ?? node.endpoint_id.slice(0,12)}</h2><p className="text-[10px] text-text-muted">{host?.architecture ?? 'Architecture unavailable'} · {host?.gpus.map(g=>g.name).filter((v,i,a)=>a.indexOf(v)===i).join(', ') || 'GPU telemetry unavailable'}</p></div><span className={`rounded px-2 py-1 text-xs ${live(node)?'bg-status-healthy/10 text-status-healthy':'bg-status-cooling/10 text-status-cooling'}`}>{live(node)?'Live':!node.enabled?'Disabled':'Stale / unavailable'}</span></div>
      <div className="flex flex-wrap gap-1">{assigned.length?assigned.map(d=><a key={d.id} href="#/models" className="rounded bg-accent/10 px-2 py-1 text-[10px] text-accent">{d.model} · {d.deployment.observed?.status?.phase ?? 'Awaiting host'}</a>):<span className="text-xs text-text-muted">No desired inference workload</span>}</div>
      <div className="flex flex-wrap"><StatChip label="CPU" value={percent(host?.cpu?.utilization_percent)} /><StatChip label="RAM used" value={gib(host?.memory?.used_bytes)} /><StatChip label="RAM total" value={gib(host?.memory?.total_bytes)} /></div>
      <LineChart series={utilizationSeries} height={135} unit="%" /><ChartLegend series={utilizationSeries} />
      <LineChart series={networkSeries} height={115} unit=" MiB/s" /><ChartLegend series={networkSeries} /><p className="text-[10px] text-text-muted">Traffic summed over non-loopback interfaces; per-interface rates are in details.</p>
      <details><summary className="cursor-pointer text-xs text-text-muted">GPU, network and container details</summary><HostTelemetry nodes={[node]} /></details>
    </Panel>;})}</div>
    <DataTable id="hosts" rows={filtered} rowKey={n=>n.endpoint_id} minWidth={800} columns={[
      {id:'host',label:'Host',required:true,render:n=>n.host_snapshot?.hostname ?? n.label ?? n.endpoint_id.slice(0,12)},
      {id:'status',label:'Status',render:n=>live(n)?'Live':'Stale / unavailable'},
      {id:'cpu',label:'CPU',render:n=>percent(n.host_snapshot?.cpu?.utilization_percent)},
      {id:'ram',label:'RAM used / total',render:n=>`${gib(n.host_snapshot?.memory?.used_bytes)} / ${gib(n.host_snapshot?.memory?.total_bytes)}`},
      {id:'models',label:'Desired models',render:n=>deployments.filter(d=>d.deployment.endpoint_id===n.endpoint_id&&d.deployment.desired_state==='ready').map(d=>d.model).join(', ')||'—'},
      {id:'sample',label:'Last sample',render:n=>n.host_snapshot?.sampled_at ?? '—'},
    ]} emptyMessage="No hosts enrolled." />
  </div>;
}
