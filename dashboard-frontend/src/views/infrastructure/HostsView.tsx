import {useState,useSyncExternalStore} from 'react';
import {useQueries} from '@tanstack/react-query';
import type {MeshNode} from '../../api/types';
import {getConnection} from '../../api/connection';
import {DataTable} from '../../components/ui/DataTable';
import {Panel} from '../../components/ui/Panel';
import {Button} from '../../components/ui/Button';
import {StatChip} from '../../components/ui/StatChip';
import {StatusIcon} from '../../components/ui/StatusIcon';
import {LineChart,ChartLegend,type ChartSeries} from '../../viz/LineChart';
import {HostTelemetry} from '../providers/HostTelemetry';
import {deploymentRows} from './managementModel';
import {useManagement} from './useManagement';
import {HOST_METRICS,HISTORY_RANGES,historyPoints,type HostMetric} from './hostHistoryModel';
const percent=(n:number|null|undefined)=>n==null?'—':`${n.toFixed(1)}%`;const gib=(n:number|null|undefined)=>n==null?'—':`${(n/1024**3).toFixed(1)} GiB`;
function live(node:MeshNode){const age=Date.now()-Date.parse(node.host_snapshot?.sampled_at??'');return node.enabled&&Number.isFinite(age)&&age>=-30_000&&age<=20_000;}
const subscribe=(callback:()=>void)=>{window.addEventListener('hashchange',callback);return()=>window.removeEventListener('hashchange',callback);};
export function HostsView(){
  const {nodes,state,error,loading}=useManagement();const {client}=getConnection();
  const selected=useSyncExternalStore(subscribe,()=>decodeURIComponent(window.location.hash.split('/')[2]??''),()=> '');
  const [range,setRange]=useState('hour');const [metric,setMetric]=useState<HostMetric>('gpu');const [hidden,setHidden]=useState<string[]>([]);const [query,setQuery]=useState('');
  const [customStart,setCustomStart]=useState('');const [customEnd,setCustomEnd]=useState('');
  const [anchor,setAnchor]=useState(Date.now());
  const until=range==='custom'&&customEnd?new Date(customEnd).getTime():anchor;
  const since=range==='custom'&&customStart?new Date(customStart).getTime():until-(HISTORY_RANGES.find(r=>r.id===range)?.ms??3600_000);
  const valid=Number.isFinite(since)&&Number.isFinite(until)&&since>=0&&since<until&&until-since<=32*86400_000;
  const filtered=nodes.filter(n=>`${n.host_snapshot?.hostname??''} ${n.label??''}`.toLowerCase().includes(query.toLowerCase()));
  const graphNodes=filtered.filter(n=>!selected||n.endpoint_id===selected);
  const history=useQueries({queries:graphNodes.map(n=>({queryKey:['host-history',n.endpoint_id,since,until],queryFn:()=>client.hostHistory(n.endpoint_id,range==='custom'?since:Date.now()-(HISTORY_RANGES.find(r=>r.id===range)?.ms??3600_000),range==='custom'?until:undefined),enabled:valid,retry:false,refetchInterval:range==='custom'?false:15000}))});
  const deployments=deploymentRows(state,nodes);const host=nodes.find(n=>n.endpoint_id===selected);
  const series:ChartSeries[]=graphNodes.filter(n=>!hidden.includes(n.endpoint_id)).map(n=>{const data=history[graphNodes.indexOf(n)]?.data;return {key:n.endpoint_id,label:n.host_snapshot?.hostname??n.label??n.endpoint_id.slice(0,12),points:historyPoints(data?.samples??[],metric,data?.bucket_ms??15000)};});
  return <div className="min-w-0 flex-1 overflow-auto p-3 sm:p-4 space-y-4" data-testid="hosts-view">
    <header className="flex flex-wrap items-start justify-between gap-3"><div><h1 className="text-lg font-semibold">{host?host.host_snapshot?.hostname??host.label:'Hosts'}</h1><p className="text-xs text-text-muted">Stored Fleet history · observations begin when history collection is enabled</p></div><div className="flex gap-2">{selected&&<Button variant="ghost" onClick={()=>{window.location.hash='#/hosts';}}>All hosts</Button>}<Button variant="ghost" onClick={()=>setAnchor(Date.now())}>Refresh range</Button></div></header>
    <Panel className="flex flex-wrap"><StatChip label="Hosts" value={nodes.length}/><StatChip label="Reporting" value={nodes.filter(live).length} accent="healthy"/><StatChip label="Inference ready" value={deployments.filter(d=>d.deployment.observed?.status?.phase==='ready').length} accent="accent"/><StatChip label="Stale / unavailable" value={nodes.filter(n=>!live(n)).length} accent="cooling"/></Panel>
    {(error||history.some(q=>q.error))&&<p role="alert">{String(error??history.find(q=>q.error)?.error)}</p>}{loading&&<p>Loading host observations…</p>}
    <Panel className="space-y-3 p-3"><div className="flex flex-wrap gap-3 items-center"><label className="text-xs">History <select aria-label="Host history range" value={range} onChange={e=>{setRange(e.target.value);setAnchor(Date.now());}} className="ml-2 rounded border border-line bg-panel p-2">{HISTORY_RANGES.map(r=><option key={r.id} value={r.id}>{r.label}</option>)}<option value="custom">Custom range</option></select></label><label className="text-xs">Metric <select aria-label="Host graph metric" value={metric} onChange={e=>setMetric(e.target.value as HostMetric)} className="ml-2 rounded border border-line bg-panel p-2">{Object.entries(HOST_METRICS).map(([id,m])=><option key={id} value={id}>{m.label}</option>)}</select></label>{range==='custom'&&<><input aria-label="History start" type="datetime-local" value={customStart} onChange={e=>setCustomStart(e.target.value)} className="rounded border border-line bg-panel p-2 text-xs"/><input aria-label="History end" type="datetime-local" value={customEnd} onChange={e=>setCustomEnd(e.target.value)} className="rounded border border-line bg-panel p-2 text-xs"/></>}</div>
      {!valid&&<p role="alert" className="text-xs">Select a time range of at most 32 days.</p>}
      <div className="flex flex-wrap gap-x-3 gap-y-1">{graphNodes.map(n=><label key={n.endpoint_id} className="text-[10px] flex items-center gap-1"><input type="checkbox" checked={!hidden.includes(n.endpoint_id)} onChange={()=>setHidden(ids=>ids.includes(n.endpoint_id)?ids.filter(id=>id!==n.endpoint_id):[...ids,n.endpoint_id])}/>{n.host_snapshot?.hostname??n.label}</label>)}</div>
      <LineChart series={series} width={1000} height={220} unit={HOST_METRICS[metric].unit} formatX={time=>new Date(time).toLocaleString(undefined,{month:'short',day:'numeric',hour:'2-digit',minute:'2-digit'})}/><ChartLegend series={series}/>
      <p className="text-[10px] text-text-muted">One stored observation per chart interval; gaps remain gaps. GPU readings average measured devices. GB10 uses system RAM, so dedicated GPU memory remains unavailable. Network sums non-loopback interfaces.</p>
    </Panel>
    {host?<><Panel className="p-3 space-y-2"><h2 className="font-semibold">Serving on this host</h2>{deployments.filter(d=>d.deployment.endpoint_id===host.endpoint_id).map(d=><a key={d.id} className="block text-xs text-accent" href="#/models">{d.model} · {d.deployment.recipe_id} · <StatusIcon status={d.deployment.observed?.status?.phase??'unknown'}/></a>)}<p className="text-xs text-text-muted">Endpoint {host.endpoint_id}</p></Panel><HostTelemetry nodes={[host]}/></>:<DataTable interactive id="hosts" rows={filtered} rowKey={n=>n.endpoint_id} minWidth={850} onRowClick={n=>{window.location.hash=`#/hosts/${encodeURIComponent(n.endpoint_id)}`;}} filterBar={<input aria-label="Search hosts" placeholder="Find a host…" value={query} onChange={e=>setQuery(e.target.value)} className="rounded border border-line bg-panel p-2 text-xs"/>} columns={[
      {id:'host',label:'Host',required:true,render:n=><button className="text-left font-semibold text-accent" onClick={()=>{window.location.hash=`#/hosts/${encodeURIComponent(n.endpoint_id)}`;}}>{n.host_snapshot?.hostname??n.label??n.endpoint_id.slice(0,12)}</button>},
      {id:'status',label:'Status',value:n=>live(n)?'Live':'Stale / unavailable',render:n=><StatusIcon status={live(n)?'live':'stale'}/>},
      {id:'cpu',label:'CPU',value:n=>n.host_snapshot?.cpu?.utilization_percent,render:n=>percent(n.host_snapshot?.cpu?.utilization_percent)},
      {id:'ram',label:'RAM used / total',value:n=>n.host_snapshot?.memory?.used_bytes,render:n=>`${gib(n.host_snapshot?.memory?.used_bytes)} / ${gib(n.host_snapshot?.memory?.total_bytes)}`},
      {id:'gpu',label:'GPU',value:n=>n.host_snapshot?HOST_METRICS.gpu.read(n.host_snapshot):null,render:n=>percent(n.host_snapshot?HOST_METRICS.gpu.read(n.host_snapshot):null)},
      {id:'models',label:'Models',render:n=>deployments.filter(d=>d.deployment.endpoint_id===n.endpoint_id&&d.deployment.desired_state==='ready').map(d=>d.model).join(', ')||'—'},
      {id:'sample',label:'Last sample',render:n=>n.host_snapshot?.sampled_at??'—'}
    ]} emptyMessage="No hosts enrolled."/>}
  </div>;
}
