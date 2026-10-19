import {useMemo,useState,type ReactNode} from 'react';
import {useQuery} from '@tanstack/react-query';
import {getConnection} from '../../api/connection';
import type {HistoryRequest} from '../../api/types';
import {Button} from '../ui/Button';
import {PanelHeader} from '../ui/Panel';
import {DataTable} from '../ui/DataTable';
import {JsonEditor} from '../ui/JsonEditor';
import {assembleOutput} from './assembledOutput';
import {AssembledOutputPanel} from './AssembledOutputPanel';
import {bodyPresentation,decodeExact,exchanges,headerValue,legacyResponse,legacyHeaders,partFor} from './wireModel';
const TABS=[['headers','Headers'],['inbound','Inbound request'],['upstream','Upstream request'],['upstream-response','Upstream response'],['response','Inbound response'],['output','Output']] as const;
export function RequestBodyTabs({request,extras=[],onTabChange,allowBodies=true,snapshot=false,fallbackHeaders,outputFallback}:{request:Pick<HistoryRequest,'id'|'status'>;allowBodies?:boolean;snapshot?:boolean;fallbackHeaders?:Record<string,string>;outputFallback?:ReactNode;extras?:Array<{id:string;label:string;content:ReactNode}>;onTabChange?:(tab:string)=>void}){
 const {client}=getConnection();const [tab,setTab]=useState<string>('inbound');const [selectedExchange,setExchange]=useState('');const [copyState,setCopy]=useState('');const [rawView,setRawView]=useState(false);
 const manifest=useQuery({queryKey:['history','requests',request.id,'wire'],queryFn:()=>client.wireManifest(request.id),enabled:allowBodies,retry:false,gcTime:30_000,refetchInterval:query=>request.status==='running'||(query.state.data&&Object.values(query.state.data.parts).some(p=>!p.complete&&!p.error))?1000:false});
 const legacy=manifest.isSuccess&&manifest.data===null;
 const stored=useQuery({queryKey:['history','requests',request.id,'events'],queryFn:()=>client.storedRequestDetail(request.id),enabled:legacy&&allowBodies,retry:false,refetchInterval:request.status==='running'?1000:false,gcTime:30_000});
 const sends=exchanges(manifest.data);const exchange=sends.includes(selectedExchange)?selectedExchange:sends.at(-1)??'';const part=partFor(tab,exchange);const record=manifest.data?.parts[part];
 const hop=tab==='inbound'?'client_in':tab==='upstream'?'upstream_out':tab==='upstream-response'?'upstream_in':'client_out';
 const isBody=tab!=='headers'&&!extras.some(t=>t.id===tab);
 const body=useQuery({queryKey:['history','requests',request.id,'wire-body',legacy?hop:part,legacy,record?.complete,record?.sha256,legacy&&tab!=='inbound'&&tab!=='upstream'?stored.dataUpdatedAt:0],queryFn:async()=>{
  if(!legacy)return client.historyBodyBytes(request.id,part);
  if(tab==='inbound'||tab==='upstream')return client.historyBodyBytes(request.id,hop,false);
  return legacyResponse(stored.data,hop)??null;
 },enabled:allowBodies&&isBody&&((!!record&&!legacy)||(legacy&&((tab==='inbound'||tab==='upstream')||stored.isSuccess))),retry:false,gcTime:30_000,refetchInterval:record&&!record.complete&&!record.error?1000:false});
 const text=useMemo(()=>body.data?decodeExact(body.data):null,[body.data]);
 const presentation=useMemo(()=>text===null||tab==='output'?null:bodyPresentation(text),[text,tab]);
 const assembled=useMemo(()=>{if(tab!=='output'||text===null)return null;const parsed=assembleOutput(text);return {...parsed,partial:parsed.partial||(!parsed.terminal&&(!!record?.error||record?.complete===false))};},[tab,text,record?.error,record?.complete]);
 const headerRows=useMemo(()=>snapshot?[]:legacy?(legacyHeaders(stored.data).length?legacyHeaders(stored.data):Object.entries(fallbackHeaders??{}).map(([name,value])=>({id:name,part:'client_in (legacy)',name,value}))):Object.entries(manifest.data?.parts??{}).flatMap(([part,p])=>p.headers.map((h,index)=>({id:part+index,part,name:h.name,value:headerValue(h.value_base64)}))),[manifest.data,legacy,stored.data,snapshot,fallbackHeaders]);
 async function copy(){if(text===null)return;try{await navigator.clipboard.writeText(text);setCopy('Copied');}catch{setCopy('Clipboard unavailable; use Download.');}}
 const download=()=>{if(!body.data)return;const url=URL.createObjectURL(new Blob([body.data as BlobPart],{type:'application/octet-stream'}));const a=document.createElement('a');a.href=url;a.download=`${request.id}-${legacy?hop:part}.body`;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);};
 if(!allowBodies)return <p className="p-3 text-xs text-text-muted">This request is not present in the selected snapshot.</p>;
 return <div className="flex min-h-0 flex-1 flex-col" data-testid="request-body-tabs">
  <div role="tablist" aria-label="Request details" className="flex shrink-0 flex-wrap gap-1 border-y border-line bg-panel-raised p-2">{[...TABS,...extras.map(t=>[t.id,t.label] as const)].map(([id,label])=><Button key={id} role="tab" aria-selected={tab===id} variant={tab===id?'default':'ghost'} onClick={()=>{setTab(id);setCopy('');onTabChange?.(id);}}>{label}</Button>)}</div>
  <div className="flex min-h-0 flex-1 flex-col gap-3 p-3" role="tabpanel" data-testid={`tabpanel-${tab}`}>
   {manifest.isError?<p role="alert">Unable to read exact capture: {String(manifest.error)}.</p>:manifest.isPending?<p>Loading stored capture…</p>:null}
   {legacy&&<p className="text-xs text-status-cooling" data-testid="legacy-wire-notice">Legacy capture: this record was reconstructed/redacted by the old logger. The original wire bytes were not retained.</p>}
   {extras.some(t=>t.id===tab)?extras.find(t=>t.id===tab)?.content:tab==='output'?body.isFetching&&!body.data?<p>Loading stored output…</p>:<AssembledOutputPanel output={assembled} fallback={outputFallback}/>:tab==='headers'?<><PanelHeader label="HTTP headers" help="Header names and byte values observed at the gateway HTTP boundary. Body tabs preserve the exact HTTP entity bytes, including compression and SSE framing. Retries and additional upstream calls are retained separately."><h3 className="text-sm font-semibold">HTTP headers</h3></PanelHeader><div className="min-h-0 overflow-auto">{headerRows.length===0&&<p data-testid="headers-empty" className="text-xs text-text-muted">No headers recorded for this view.</p>}<DataTable id="wire-headers" interactive rows={headerRows} rowKey={r=>r.id} columns={[{id:'part',label:'Hop',value:r=>r.part,render:r=>r.part},{id:'name',label:'Header',value:r=>r.name,render:r=>r.name},{id:'value',label:'Value',value:r=>r.value,render:r=><span className="break-all font-mono">{r.value}</span>}]}/></div></>:<>
    <PanelHeader label="Stored HTTP body" help="Copy and download use the original stored bytes. Valid JSON is formatted for display only. Copy and Download retain the original bytes without redaction or truncation. Storage may share identical byte chunks. A binary or compressed entity can be downloaded without decoding it." actions={<>{presentation?.language==='json'&&<Button variant="ghost" onClick={()=>setRawView(!rawView)}>{rawView?'Format JSON':'Show raw'}</Button>}<Button disabled={text===null||!!record&& !record.complete&&!record.error} title={record&&!record.complete&&!record.error?'Capture is still recording':undefined} onClick={()=>void copy()}>{legacy?'Copy legacy capture':'Copy body'}</Button><Button variant="ghost" disabled={!body.data||!!record&&!record.complete&&!record.error} onClick={download}>Download</Button></>}><div className="text-xs text-text-muted">{snapshot?'Stored body':record?`${record.complete?'Exact capture':record.error?'Partial transport':'Recording…'} · ${(body.data?.byteLength??record.bytes).toLocaleString()} bytes`:legacy?'Legacy stored body':'No body recorded for this hop'}</div></PanelHeader>
    {(tab==='upstream'||tab==='upstream-response')&&sends.length>1&&<label className="flex items-center gap-2 text-xs">Upstream call<select aria-label="Upstream call" className="min-w-0 rounded border border-line bg-bg p-2" value={exchange} onChange={e=>{setExchange(e.target.value);setCopy('');}}>{sends.map((value,index)=><option key={value} value={value}>Call {index+1} · {manifest.data?.parts[value+'_out']?.target??'upstream'} · {manifest.data?.parts[value+'_in']?.status??'pending'}</option>)}</select></label>}
    {copyState&&<p role="status" className="text-xs">{copyState}</p>}
    {record?.error&&<p role="status" className="text-xs">{record.error}</p>}
    {body.isError?<p role="alert">Could not load the stored body. {String(body.error)}</p>:body.isFetching&&!body.data?<p>Loading complete body…</p>:text!==null?<JsonEditor value={rawView?text:presentation!.value} onChange={()=>{}} readOnly label={TABS.find(t=>t[0]===tab)?.[1]??'HTTP body'} language={rawView?'plaintext':presentation!.language} fill/>:body.data?<p className="text-xs">Binary or compressed body. Download preserves every byte.</p>:<p className="text-xs text-text-muted">No body was recorded for this hop.</p>}
   </>}
  </div>
 </div>;
}
