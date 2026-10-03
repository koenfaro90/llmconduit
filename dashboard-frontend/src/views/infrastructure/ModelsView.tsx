import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { getConnection } from '../../api/connection';
import { DataTable } from '../../components/ui/DataTable';
import { Panel } from '../../components/ui/Panel';
import { StatChip } from '../../components/ui/StatChip';
import { deploymentRows } from './managementModel';
import { useManagement } from './useManagement';
export function ModelsView() {
  const {nodes,state,error}=useManagement();const {client}=getConnection();const [query,setQuery]=useState('');const [expanded,setExpanded]=useState<string[]>([]);
  const activity=useQuery({queryKey:['model-activity'],queryFn:()=>client.modelActivity(),retry:false,refetchInterval:5_000});
  const rows=deploymentRows(state,nodes).filter(r=>r.deployment.desired_state==='ready'||r.deployment.observed?.status?.phase==='ready').filter(r=>`${r.model} ${r.id} ${r.host}`.toLowerCase().includes(query.toLowerCase())).map(row=>{
    const matches=activity.data?.filter(a=>a.model===row.model) ?? [];return {...row,last:matches.length?Math.max(...matches.map(a=>a.last_request_at_ms)):null,counts:activity.data?Array.from({length:5},(_,i)=>matches.reduce((sum,a)=>sum+a.requests[i]!,0)):null};
  });
  return <div className="min-w-0 flex-1 overflow-auto p-4 space-y-4" data-testid="models-view"><header className="flex flex-wrap justify-between gap-3"><div><h1 className="text-lg font-semibold">Models</h1><p className="text-xs text-text-muted">Running and desired inference workloads, pinned configuration and gateway request activity</p></div><input aria-label="Search models" placeholder="Find a model or host…" value={query} onChange={e=>setQuery(e.target.value)} className="rounded border border-line bg-panel p-2 text-xs" /></header>
    <Panel className="flex flex-wrap"><StatChip label="Models" value={new Set(rows.map(r=>r.model)).size} /><StatChip label="Ready containers" value={rows.filter(r=>r.deployment.observed?.status?.phase==='ready').length} accent="healthy" /><StatChip label="Hosts serving" value={new Set(rows.map(r=>r.host)).size} accent="accent" /></Panel>
    <p className="text-xs text-text-muted">Request counts use retained SQL history and exclude health/catalog polling. Activity totals are per served model across its gateway backends; direct LiteLLM/backend requests are outside this gateway's observations.</p>
    {(error||activity.error)&&<p role="alert">{String(error??activity.error)}</p>}
    <DataTable id="running-models" tableTestId="running-models" rows={rows} rowKey={r=>r.id} minWidth={1200} columns={[
      {id:'model',label:'Model',required:true,render:r=><div><strong>{r.model}</strong><p className="text-[10px] text-text-muted">{r.id}</p></div>},
      {id:'host',label:'Host',render:r=>r.host},
      {id:'recipe',label:'Recipe / revision',render:r=>`${r.deployment.recipe_id} · ${r.revision?.revision ?? '—'}`},
      {id:'state',label:'State',render:r=><span>{r.deployment.observed?.status?.phase ?? 'Awaiting host'}{r.deployment.observed_at_ms==null||Date.now()-r.deployment.observed_at_ms>20_000?' (stale)':''}</span>},
      {id:'last',label:'Last inference request',render:r=>r.last==null?'—':new Date(r.last).toLocaleString()},
      ...['1 min','15 min','1 hour','4 hours','24 hours'].map((label,i)=>({id:`requests-${i}`,label,align:'right' as const,render:(r:typeof rows[number])=>r.counts?.[i] ?? '—'})),
    ]} expandedKeys={expanded} onToggleExpanded={r=>setExpanded(old=>old.includes(r.id)?old.filter(id=>id!==r.id):[...old,r.id])} canExpand={()=>true} expandOnRowClick renderExpanded={r=><Panel className="p-3"><div className="flex flex-wrap gap-4 text-xs"><span>Image: {String(r.revision?.model_spec.image ?? '—')}</span><span>GPUs: {String(r.revision?.model_spec.gpus ?? '—')}</span><span>RAM: {String(r.revision?.model_spec.memory_limit ?? '—')}</span><span>User: {String(r.revision?.model_spec.user ?? '—')}</span></div><pre className="mt-3 max-h-96 overflow-auto text-xs">{JSON.stringify(r.revision?.model_spec ?? {},null,2)}</pre></Panel>} emptyMessage="No running or desired inference workloads." />
  </div>;
}
