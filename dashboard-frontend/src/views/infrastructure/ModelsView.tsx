import {useState} from 'react';
import {useQuery} from '@tanstack/react-query';
import {getConnection} from '../../api/connection';
import {DataTable} from '../../components/ui/DataTable';
import {Button} from '../../components/ui/Button';
import {StatusIcon} from '../../components/ui/StatusIcon';
import {Panel} from '../../components/ui/Panel';
import {StatChip} from '../../components/ui/StatChip';
import {runtimeSpec,deploymentRows,type DeploymentRow} from './managementModel';
import {useManagement} from './useManagement';
interface ModelRow extends DeploymentRow {aggregate:boolean;children:ModelRow[];last:number|null;counts:number[]|null}
export function ModelsView(){
  const {nodes,state,error}=useManagement();const {client}=getConnection();const [query,setQuery]=useState('');const [expanded,setExpanded]=useState<string[]>([]);const [details,setDetails]=useState<DeploymentRow|null>(null);
  const activity=useQuery({queryKey:['model-activity'],queryFn:()=>client.modelActivity(),retry:false,refetchInterval:5000});
  const deployments=deploymentRows(state,nodes).filter(r=>r.deployment.desired_state==='ready'||r.deployment.observed?.status?.phase==='ready');
  const groups=new Map<string,DeploymentRow[]>();for(const row of deployments){groups.set(row.model,[...(groups.get(row.model)??[]),row]);}
  const rows:ModelRow[]=[...groups].map(([model,hosts])=>{const matches=activity.data?.filter(a=>a.model===model)??[];const last=matches.length?Math.max(...matches.map(a=>a.last_request_at_ms)):null;const counts=activity.data?Array.from({length:5},(_,i)=>matches.reduce((sum,a)=>sum+a.requests[i]!,0)):null;
    return {...hosts[0]!,model,id:`model:${model}`,aggregate:true,last,counts,host:`${new Set(hosts.map(h=>h.host)).size} hosts`,children:hosts.map(row=>({...row,aggregate:false,children:[],last:null,counts:null}))};
  }).filter(r=>`${r.model} ${r.children.map(h=>h.host).join(' ')}`.toLowerCase().includes(query.toLowerCase()));
  return <div className="min-w-0 flex-1 overflow-auto p-3 sm:p-4 space-y-4" data-testid="models-view"><header><h1 className="text-lg font-semibold">Models</h1><p className="text-xs text-text-muted">One row per served model · expand to inspect every host and pinned recipe</p></header><Panel className="flex flex-wrap"><StatChip label="Models" value={groups.size}/><StatChip label="Ready containers" value={deployments.filter(r=>r.deployment.observed?.status?.phase==='ready').length} accent="healthy"/><StatChip label="Serving hosts" value={new Set(deployments.map(r=>r.host)).size}/></Panel>
    <p className="text-xs text-text-muted">Activity totals use SQL gateway history across all backends. Direct LiteLLM calls and health polling are excluded. Per-host attribution is unavailable when the gateway history cannot identify a rank.</p>{(error||activity.error)&&<p role="alert">{String(error??activity.error)}</p>}
    <DataTable interactive id="running-models" tableTestId="running-models" rows={rows} rowKey={r=>r.id} minWidth={1250} filterBar={<input aria-label="Search models" placeholder="Find a model or host…" value={query} onChange={e=>setQuery(e.target.value)} className="rounded border border-line bg-panel p-2 text-xs"/>} columns={[
      {id:'model',label:'Model',required:true,render:r=>r.aggregate?<strong>{r.model}</strong>:<span className="pl-3 text-text-muted">↳ {r.host}</span>,value:r=>r.aggregate?r.model:r.host},
      {id:'host',label:'Placement',value:r=>r.aggregate?r.children.map(c=>c.host).join(', '):r.host,render:r=>r.aggregate?r.host:<a className="text-accent" href={`#/hosts/${r.deployment.endpoint_id}`}>{r.host}</a>},
      {id:'recipe',label:'Recipe / revision',render:r=>r.aggregate?[...new Set(r.children.map(c=>c.deployment.recipe_id))].join(', '):`${r.deployment.recipe_id} · ${r.revision?.revision??'—'}`},
      {id:'state',label:'State',value:r=>r.aggregate?r.children.every(c=>c.deployment.observed?.status?.phase==='ready')?'ready':'pending':r.deployment.observed?.status?.phase??'unknown',render:r=><StatusIcon status={r.aggregate?r.children.every(c=>c.deployment.observed?.status?.phase==='ready')?'ready':'pending':r.deployment.observed_at_ms==null||Date.now()-r.deployment.observed_at_ms>20_000?'stale':r.deployment.observed?.status?.phase??'unknown'}/>},
      {id:'last',label:'Last inference request',value:r=>r.last,render:r=>r.last==null?'—':new Date(r.last).toLocaleString()},
      ...['1 min','15 min','1 hour','4 hours','24 hours'].map((label,i)=>({id:`requests-${i}`,label,align:'right' as const,value:(r:ModelRow)=>r.counts?.[i],render:(r:ModelRow)=>r.counts?.[i]??'—'})),
      {id:'configuration',label:'Configuration',render:r=><Button variant="ghost" onClick={e=>{e.stopPropagation();setDetails(r.aggregate?r.children[0]!:r);}}>Review</Button>}
    ]} canExpand={r=>r.aggregate} expandedRows={r=>r.children} expandedKeys={expanded} expandOnRowClick onToggleExpanded={r=>setExpanded(ids=>ids.includes(r.id)?ids.filter(id=>id!==r.id):[...ids,r.id])} emptyMessage="No running or desired inference models."/>
    {details&&<Panel className="p-3 space-y-3"><div className="flex justify-between gap-2"><h2 className="text-sm font-semibold">{details.model} · {details.host}</h2><Button variant="ghost" onClick={()=>setDetails(null)}>Close</Button></div><p className="text-xs break-all">Image: {String(runtimeSpec(details).image??'—')}</p><p className="text-xs">Deployment {details.id} · recipe {details.deployment.recipe_id} · revision {details.revision?.revision??'—'}</p><pre className="max-h-96 overflow-auto text-xs">{JSON.stringify(runtimeSpec(details),null,2)}</pre></Panel>}
  </div>;
}
