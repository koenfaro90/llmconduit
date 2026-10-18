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
import {HostArtifactsPanel} from './HostArtifactsPanel';
import {deploymentRows} from './managementModel';
import {sortHosts,hostName,shortHost,hardwareLabel,vramLabel,sampleTime,compareNames} from './hardwareModel';
import {seriesColor} from '../../viz/seriesColor';
import {useManagement} from './useManagement';
import {HOST_METRICS,HISTORY_RANGES,historyPoints,GPU_METRIC_IDS,FILESYSTEM_METRIC_IDS,BLOCK_DEVICE_METRIC_IDS,type HostMetric} from './hostHistoryModel';
const percent=(n:number|null|undefined)=>n==null?'—':`${n.toFixed(1)}%`;const gib=(n:number|null|undefined)=>n==null?'—':`${(n/1024**3).toFixed(1)} GiB`;
function live(node:MeshNode){const age=Date.now()-Date.parse(node.host_snapshot?.sampled_at??'');return node.enabled&&Number.isFinite(age)&&age>=-30_000&&age<=20_000;}
const subscribe=(callback:()=>void)=>{window.addEventListener('hashchange',callback);return()=>window.removeEventListener('hashchange',callback);};
export function HostsView(){
  const {nodes:unsortedNodes,state,error,loading}=useManagement();const nodes=sortHosts(unsortedNodes);const {client}=getConnection();
  const selected=useSyncExternalStore(subscribe,()=>decodeURIComponent(window.location.hash.split('/')[2]??''),()=> '');
  const [range,setRange]=useState('hour');const [metrics,setMetrics]=useState<HostMetric[]>(['gpu']);const [hidden,setHidden]=useState<string[]>([]);const [query,setQuery]=useState('');
  const [customStart,setCustomStart]=useState('');const [customEnd,setCustomEnd]=useState('');
  const [anchor,setAnchor]=useState(Date.now());const [hiddenGPUs,setHiddenGPUs]=useState<string[]>([]);const [hiddenStorage,setHiddenStorage]=useState<string[]>([]);
  const until=range==='custom'&&customEnd?new Date(customEnd).getTime():anchor;
  const since=range==='custom'&&customStart?new Date(customStart).getTime():until-(HISTORY_RANGES.find(r=>r.id===range)?.ms??3600_000);
  const valid=Number.isFinite(since)&&Number.isFinite(until)&&since>=0&&since<until&&until-since<=32*86400_000;
  const filtered=nodes.filter(n=>`${n.host_snapshot?.hostname??''} ${n.label??''}`.toLowerCase().includes(query.toLowerCase()));
  const graphNodes=filtered.filter(n=>!selected||n.endpoint_id===selected);
  const history=useQueries({queries:graphNodes.map(n=>({queryKey:['host-history',n.endpoint_id,since,until],queryFn:()=>client.hostHistory(n.endpoint_id,range==='custom'?since:Date.now()-(HISTORY_RANGES.find(r=>r.id===range)?.ms??3600_000),range==='custom'?until:undefined),enabled:valid,retry:false,refetchInterval:range==='custom'?false:15000}))});
  const deployments=deploymentRows(state,nodes);const host=nodes.find(n=>n.endpoint_id===selected);
  const visible=graphNodes.filter(n=>!hidden.includes(n.endpoint_id));
  const units=[...new Set(metrics.map(m=>HOST_METRICS[m].unit))];
  const groups=[...new Set(graphNodes.map(hardwareLabel))].sort(compareNames);
  const charts=units.map(unit=>({unit,metrics:metrics.filter(m=>HOST_METRICS[m].unit===unit),series:visible.flatMap(n=>metrics.filter(m=>HOST_METRICS[m].unit===unit).flatMap((metric,index):ChartSeries[]=>{
    const data=history[graphNodes.indexOf(n)]?.data;const individual=Boolean(selected)&&GPU_METRIC_IDS.includes(metric);
    const fsMetric=Boolean(selected)&&(FILESYSTEM_METRIC_IDS as readonly HostMetric[]).includes(metric);
    const ioMetric=Boolean(selected)&&(BLOCK_DEVICE_METRIC_IDS as readonly HostMetric[]).includes(metric);
    // Include historical devices so unmounted/replaced storage keeps its history.
    const filesystemRows=[...new Map([...(data?.samples??[]).flatMap(s=>s.filesystems??[]),...(n.host_snapshot?.filesystems??[])].map(f=>[f.mount_point,f])).values()].sort((a,b)=>a.mount_point.localeCompare(b.mount_point));
    const blockRows=[...new Map([...(data?.samples??[]).flatMap(s=>s.block_devices??[]),...(n.host_snapshot?.block_devices??[])].map(d=>[d.device_id,d])).values()].sort((a,b)=>a.name.localeCompare(b.name));
    const devices:Array<{key:string;label:string;index:number;uuid?:string;storage?:{filesystem?:string;device?:string}}>=individual?[...(n.host_snapshot?.gpus??[])].sort((a,b)=>a.index-b.index).filter(g=>!hiddenGPUs.includes(g.uuid)).map(g=>({key:g.uuid,label:`GPU ${g.index}`,index:g.index,uuid:g.uuid})):fsMetric?filesystemRows.map((f,i)=>({key:`fs:${f.mount_point}`,label:f.mount_point,index:i,storage:{filesystem:f.mount_point}})).filter(d=>!hiddenStorage.includes(d.key)):ioMetric?blockRows.map((d,i)=>({key:`io:${d.device_id}`,label:d.name,index:i,storage:{device:d.device_id}})).filter(d=>!hiddenStorage.includes(d.key)):[{key:'host',label:'',index:nodes.indexOf(n)}];
    return devices.map(device=>({key:`${n.endpoint_id}:${metric}:${device.key}`,label:`${shortHost(hostName(n))}${device.label?` · ${device.label}`:''} · ${HOST_METRICS[metric].label}`,stroke:seriesColor(device.index),dash:[undefined,'6 3','2 3','8 3 2 3'][index%4],points:historyPoints(data?.samples??[],metric,data?.bucket_ms??15000,device.uuid,device.storage)}));
  }))}));
  const hostHistory=host?history[graphNodes.findIndex(n=>n.endpoint_id===host.endpoint_id)]?.data:undefined;
  const storageChoices=host?[
    ...[...new Map([...(hostHistory?.samples??[]).flatMap(s=>s.filesystems??[]),...(host.host_snapshot?.filesystems??[])].map(f=>[f.mount_point,f])).values()].sort((a,b)=>a.mount_point.localeCompare(b.mount_point)).map(f=>({key:`fs:${f.mount_point}`,label:f.mount_point})),
    ...[...new Map([...(hostHistory?.samples??[]).flatMap(s=>s.block_devices??[]),...(host.host_snapshot?.block_devices??[])].map(d=>[d.device_id,d])).values()].sort((a,b)=>a.name.localeCompare(b.name)).map(d=>({key:`io:${d.device_id}`,label:`${d.name}${d.leaf_device?'':' (logical)'}`})),
  ]:[];
  return <div className="min-w-0 flex-1 overflow-auto p-3 sm:p-4 space-y-4" data-testid="hosts-view">
    <header className="flex flex-wrap items-start justify-between gap-3"><div><h1 className="text-lg font-semibold">{host?host.host_snapshot?.hostname??host.label:'Hosts'}</h1><p className="text-xs text-text-muted">Stored Fleet history · observations begin when history collection is enabled</p></div><div className="flex gap-2">{selected&&<Button variant="ghost" onClick={()=>{window.location.hash=`#/host-settings/${encodeURIComponent(selected)}`;}}>Host settings</Button>}{selected&&<Button variant="ghost" onClick={()=>{window.location.hash='#/hosts';}}>All hosts</Button>}<Button variant="ghost" onClick={()=>setAnchor(Date.now())}>Refresh range</Button></div></header>
    <Panel className="flex flex-wrap"><StatChip label="Hosts" value={nodes.length}/><StatChip label="Reporting" value={nodes.filter(live).length} accent="healthy"/><StatChip label="Inference ready" value={deployments.filter(d=>d.deployment.observed?.status?.phase==='ready').length} accent="accent"/><StatChip label="Stale / unavailable" value={nodes.filter(n=>!live(n)).length} accent="cooling"/></Panel>
    {(error||history.some(q=>q.error))&&<p role="alert">{String(error??history.find(q=>q.error)?.error)}</p>}{loading&&<p>Loading host observations…</p>}
    <Panel className="space-y-3 p-3"><div className="flex flex-wrap gap-3 items-center"><label className="text-xs">History <select aria-label="Host history range" value={range} onChange={e=>{setRange(e.target.value);setAnchor(Date.now());}} className="ml-2 rounded border border-line bg-panel p-2">{HISTORY_RANGES.map(r=><option key={r.id} value={r.id}>{r.label}</option>)}<option value="custom">Custom range</option></select></label>{range==='custom'&&<><input aria-label="History start" type="datetime-local" value={customStart} onChange={e=>setCustomStart(e.target.value)} className="rounded border border-line bg-panel p-2 text-xs"/><input aria-label="History end" type="datetime-local" value={customEnd} onChange={e=>setCustomEnd(e.target.value)} className="rounded border border-line bg-panel p-2 text-xs"/></>}</div>
      {!valid&&<p role="alert" className="text-xs">Select a time range of at most 32 days.</p>}
      <div className="grid min-w-0 gap-4 lg:grid-cols-[220px_minmax(0,1fr)]">
        <aside className="space-y-4 rounded-lg border border-line bg-panel p-3" aria-label="Graph selectors">
          <fieldset className="space-y-2"><legend className="mb-2 font-semibold text-xs">Measures</legend>{(Object.keys(HOST_METRICS) as HostMetric[]).map(id=><label key={id} className="flex items-center gap-2 text-xs"><input type="checkbox" checked={metrics.includes(id)} onChange={()=>setMetrics(current=>current.includes(id)?current.filter(m=>m!==id):[...current,id])}/>{HOST_METRICS[id].label}<span className="ml-auto text-text-muted">{HOST_METRICS[id].unit}</span></label>)}</fieldset>
          {host&&<fieldset className="space-y-2 border-t border-line pt-2"><legend className="font-semibold text-xs">Physical GPUs</legend>{[...(host.host_snapshot?.gpus??[])].sort((a,b)=>a.index-b.index).map(gpu=><label key={gpu.uuid} title={gpu.uuid} className="flex items-center gap-2 text-xs"><input type="checkbox" checked={!hiddenGPUs.includes(gpu.uuid)} onChange={()=>setHiddenGPUs(ids=>ids.includes(gpu.uuid)?ids.filter(id=>id!==gpu.uuid):[...ids,gpu.uuid])}/><span className="h-2 w-2 rounded-full" style={{background:seriesColor(gpu.index)}}/>GPU {gpu.index}<span className="truncate text-text-muted">{gpu.name}</span></label>)}</fieldset>}
          {host&&storageChoices.length>0&&<fieldset className="space-y-2 border-t border-line pt-2"><legend className="font-semibold text-xs">Filesystems &amp; disks</legend>{storageChoices.map(device=><label key={device.key} className="flex items-center gap-2 break-all text-xs"><input aria-label={`Show ${device.label}`} type="checkbox" checked={!hiddenStorage.includes(device.key)} onChange={()=>setHiddenStorage(ids=>ids.includes(device.key)?ids.filter(id=>id!==device.key):[...ids,device.key])}/>{device.label}</label>)}</fieldset>}
          <div className="space-y-3"><h3 className="font-semibold text-xs">Hosts by GPU type</h3>{groups.map(group=>{const members=graphNodes.filter(n=>hardwareLabel(n)===group);const all=members.every(n=>!hidden.includes(n.endpoint_id));return <fieldset key={group} className="space-y-2 border-t border-line pt-2"><legend className="max-w-full pt-2 text-[11px] font-semibold"><label className="flex items-start gap-2"><input type="checkbox" checked={all} ref={element=>{if(element)element.indeterminate=!all&&members.some(n=>!hidden.includes(n.endpoint_id));}} onChange={()=>setHidden(ids=>all?[...new Set([...ids,...members.map(n=>n.endpoint_id)])]:ids.filter(id=>!members.some(n=>n.endpoint_id===id)))}/>{group}</label></legend>{members.map(n=><label key={n.endpoint_id} title={hostName(n)} className="flex items-center gap-2 pl-3 text-xs"><input type="checkbox" checked={!hidden.includes(n.endpoint_id)} onChange={()=>setHidden(ids=>ids.includes(n.endpoint_id)?ids.filter(id=>id!==n.endpoint_id):[...ids,n.endpoint_id])}/><span className="h-2 w-2 rounded-full" style={{background:seriesColor(nodes.indexOf(n))}}/>{shortHost(hostName(n))}</label>)}</fieldset>;})}</div>
        </aside>
        <div className="min-w-0 space-y-4">{!metrics.length&&<p className="text-xs text-text-muted">Select measures to plot.</p>}{charts.map(chart=><div key={chart.unit} className="min-w-0 space-y-2"><h3 className="text-xs font-semibold">{chart.metrics.map(m=>HOST_METRICS[m].label).join(' · ')} <span className="text-text-muted">({chart.unit.trim()})</span></h3><LineChart series={chart.series} width={720} height={240} unit={chart.unit} formatX={time=>new Date(time).toLocaleString(undefined,{month:'short',day:'numeric',hour:'2-digit',minute:'2-digit'})}/><ChartLegend series={chart.series}/></div>)}</div>
      </div>
      <p className="text-[10px] text-text-muted">One stored observation per chart interval; gaps remain gaps. {selected?'GPU lines show each selected physical device.':'GPU readings average measured devices.'} GB10 uses system RAM, so dedicated GPU memory remains unavailable. Network sums non-loopback interfaces. PCIe sums all measured GPUs; RX is traffic entering the GPU, TX is leaving it. Rates cover the driver’s short window, not the chart interval. Storage capacity shows the root filesystem in the overview and each selected mount in host details. Disk overview totals use leaf devices only; busy is their maximum and latency is weighted by completed IOPS. Logical devices are selectable separately. Idle latency and unknown counters remain unavailable.</p>
    </Panel>
    {host?<><Panel className="p-3 space-y-2"><h2 className="font-semibold">Serving on this host</h2>{deployments.filter(d=>d.deployment.endpoint_id===host.endpoint_id).map(d=><a key={d.id} className="block text-xs text-accent" href="#/models">{d.model} · {d.deployment.recipe_id} · <StatusIcon status={d.deployment.observed?.status?.phase??'unknown'}/></a>)}<p className="text-xs text-text-muted">Endpoint {host.endpoint_id}</p></Panel><HostTelemetry nodes={[host]}/><HostArtifactsPanel key={host.endpoint_id} endpointId={host.endpoint_id}/></>:<DataTable interactive id="hosts" columnStateKey="hosts-hardware-v2" rows={filtered} rowKey={n=>n.endpoint_id} minWidth={850} onRowClick={n=>{window.location.hash=`#/hosts/${encodeURIComponent(n.endpoint_id)}`;}} filterBar={<input aria-label="Search hosts" placeholder="Find a host…" value={query} onChange={e=>setQuery(e.target.value)} className="rounded border border-line bg-panel p-2 text-xs"/>} columns={[
      {id:'host',label:'Host',required:true,render:n=><button className="text-left font-semibold text-accent" onClick={()=>{window.location.hash=`#/hosts/${encodeURIComponent(n.endpoint_id)}`;}}>{n.host_snapshot?.hostname??n.label??n.endpoint_id.slice(0,12)}</button>},
      {id:'status',label:'Status',value:n=>live(n)?'Live':'Stale / unavailable',render:n=><StatusIcon status={live(n)?'live':'stale'}/>},
      {id:'cpu',label:'CPU',value:n=>n.host_snapshot?.cpu?.utilization_percent,render:n=>percent(n.host_snapshot?.cpu?.utilization_percent)},
      {id:'ram',label:'RAM used / total',value:n=>n.host_snapshot?.memory?.used_bytes,render:n=>`${gib(n.host_snapshot?.memory?.used_bytes)} / ${gib(n.host_snapshot?.memory?.total_bytes)}`},
      {id:'disk',label:'Disk available / total',value:n=>n.host_snapshot?.disk?.available_bytes,render:n=>`${gib(n.host_snapshot?.disk?.available_bytes)} / ${gib(n.host_snapshot?.disk?.total_bytes)}`},
      {id:'vram',label:'VRAM used / total',value:n=>n.host_snapshot?.gpus.reduce((a,g)=>a+(g.memory_used_mib??0),0),render:vramLabel},
      {id:'temperature',label:'GPU temperature',value:n=>n.host_snapshot?HOST_METRICS.temperature.read(n.host_snapshot):null,render:n=>{const value=n.host_snapshot?HOST_METRICS.temperature.read(n.host_snapshot):null;return value==null?'—':`${value.toFixed(0)} °C`;}},
      {id:'gpu-type',label:'GPU type',render:hardwareLabel},
      {id:'gpu',label:'GPU',value:n=>n.host_snapshot?HOST_METRICS.gpu.read(n.host_snapshot):null,render:n=>percent(n.host_snapshot?HOST_METRICS.gpu.read(n.host_snapshot):null)},
      {id:'models',label:'Models',render:n=>deployments.filter(d=>d.deployment.endpoint_id===n.endpoint_id&&d.deployment.desired_state==='ready').map(d=>d.model).join(', ')||'—'},
      {id:'sample',label:'Last sample',value:n=>n.host_snapshot?.sampled_at,render:n=>sampleTime(n.host_snapshot?.sampled_at)}
    ]} emptyMessage="No hosts enrolled."/>}
  </div>;
}
