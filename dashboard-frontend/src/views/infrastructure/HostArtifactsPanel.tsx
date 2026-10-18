import {useState} from 'react';
import {useQuery} from '@tanstack/react-query';
import {getConnection} from '../../api/connection';
import {Panel,PanelHeader} from '../../components/ui/Panel';
import {Button} from '../../components/ui/Button';
import {DataTable} from '../../components/ui/DataTable';
const gib=(n:number)=>`${(n/1024**3).toFixed(2)} GiB`;
export function HostArtifactsPanel({endpointId}:{endpointId:string}){
 const {client}=getConnection();const [tab,setTab]=useState<'images'|'checkpoints'>('images');const query=useQuery({queryKey:['host-artifacts',endpointId],queryFn:()=>client.hostArtifacts(endpointId),enabled:false,retry:false});const inventory=query.data;
 return <Panel className="min-w-0 space-y-3 p-3" data-testid="host-artifacts-panel"><PanelHeader label="Images and model cache" help="Ask this host's Fleet agent for a read-only inventory of Docker images and its approved Hugging Face cache. Inspection lists local metadata only: it does not pull, download, read weight contents or change files. Snapshot presence does not prove completeness or hash integrity; use Deployment preparation to verify a recipe. Snapshot sizes cover top-level files only and are logical sizes, not physical disk usage. Results are bounded and refreshed only when requested." actions={<Button variant="ghost" disabled={query.isFetching} onClick={()=>void query.refetch()}>{query.isFetching?'Inspecting…':'Inspect host'}</Button>}><h2 className="font-semibold">Images and model cache</h2></PanelHeader>
 {!inventory&&!query.error&&<p className="text-xs text-text-muted">Inspect this host to see its cached images and checkpoint revisions.</p>}
 {query.error&&<p role="alert" className="text-xs">{String(query.error)}</p>}
 {inventory&&<><div className="flex flex-wrap items-center gap-2 text-xs"><Button variant="ghost" onClick={()=>setTab('images')}>Images ({inventory.images.length})</Button><Button variant="ghost" onClick={()=>setTab('checkpoints')}>Checkpoints ({inventory.snapshots.length})</Button><span className="text-text-muted">Inspected {new Date(inventory.sampled_at).toLocaleString()}</span></div>{inventory.errors.map(error=><p key={error} role="alert" className="text-xs">{error}</p>)}{inventory.truncated&&<p className="text-xs text-text-muted">Inventory reached its display bound; additional entries may exist.</p>}
 {tab==='images'?<DataTable interactive id="host-cached-images" rows={inventory.images} rowKey={r=>r.id} minWidth={750} clientPageSize={10} mobileColumns={['tags','architecture','size']} columns={[
  {id:'tags',label:'Image',value:r=>(r.tags??[]).join(' '),render:r=><span className="break-all">{(r.tags??[]).join(', ')||'Untagged'}</span>},
  {id:'id',label:'Image ID',value:r=>r.id,render:r=><code title={r.id}>{r.id.slice(0,23)}…</code>},
  {id:'digest',label:'Repository digests',value:r=>(r.digests??[]).join(' '),render:r=><span className="break-all">{(r.digests??[]).join(', ')||'—'}</span>},
  {id:'architecture',label:'Architecture',value:r=>`${r.os}/${r.architecture}`,render:r=>`${r.os}/${r.architecture}`},
  {id:'size',label:'Image size',value:r=>r.size_bytes,render:r=>gib(r.size_bytes)},
 ]} emptyMessage="No cached images reported."/>:<><p className="text-xs text-text-muted">{inventory.cache_configured?'Presence only. Prepare a pinned recipe to verify its files.':'This host has no approved checkpoint cache configured.'}</p><DataTable interactive id="host-cached-checkpoints" rows={inventory.snapshots} rowKey={r=>`${r.repository}:${r.revision}`} minWidth={750} clientPageSize={10} mobileColumns={['repository','revision','folders']} columns={[
  {id:'repository',label:'Repository',value:r=>r.repository,render:r=><a className="break-all text-accent" href={`https://huggingface.co/${r.repository}/tree/${r.revision}`} target="_blank" rel="noreferrer">{r.repository}</a>},
  {id:'revision',label:'Revision',value:r=>r.revision,render:r=><code title={r.revision}>{r.revision.slice(0,12)}</code>},
  {id:'entries',label:'Top-level entries',value:r=>r.top_level_entries,render:r=>r.top_level_entries},
  {id:'folders',label:'Subfolders',value:r=>r.subfolders.join(', ')||'—',render:r=>r.subfolders.join(', ')||'—'},
  {id:'size',label:'Top-level logical size',value:r=>r.top_level_logical_bytes,render:r=>gib(r.top_level_logical_bytes)},
  {id:'limitations',label:'Incomplete inventory',value:r=>`${r.truncated?'Bound reached; ':''}${r.unreadable_entries?`${r.unreadable_entries} unreadable entries`:''}`||'—',render:r=>`${r.truncated?'Bound reached; ':''}${r.unreadable_entries?`${r.unreadable_entries} unreadable entries`:''}`||'—'},
 ]} emptyMessage="No checkpoint snapshots reported."/></>}
 </>}
 </Panel>;
}
