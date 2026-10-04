import type {ManagementState,MeshNode} from '../../api/types';
import {shortHost,hostName} from './hardwareModel';
import {gpuEstimate,placementFit,resourceReservations} from './recipeSizes';
export function GpuSelection({node,spec,devices,onChange,state}:{node:MeshNode;spec:Record<string,unknown>;devices:number[];onChange:(devices:number[])=>void;state:ManagementState|undefined}){
 const host=shortHost(hostName(node));
 return <fieldset aria-label={`GPUs on ${host}`} className="space-y-1 border-t border-line pt-2"><legend className="text-text-muted">Physical GPUs · {devices.length} selected</legend>{node.host_snapshot?.gpus.slice().sort((a,b)=>a.index-b.index).map(gpu=>{
  const budget=node.host_snapshot?.managed_capacity?.gpus.find(g=>g.index===gpu.index);
  const held=resourceReservations(node,state).gpus.get(gpu.index);
  const fit=placementFit({...spec,gpus:`device=${gpu.index}`},node,state);const chosen=devices.includes(gpu.index);const estimate=gpuEstimate(spec,budget?.total_mib??null);
  return <label key={gpu.index} className={`flex gap-2 rounded bg-panel px-2 py-1 ${fit.fits||chosen?'':'opacity-60'}`} title={fit.reason}>
   <input type="checkbox" aria-label={`GPU ${gpu.index} on ${host}`} checked={chosen} disabled={!fit.fits&&!chosen} onChange={()=>onChange(chosen?devices.filter(i=>i!==gpu.index):[...devices,gpu.index].sort((a,b)=>a-b))}/>
   <span className="min-w-0 flex-1"><span className="block">GPU {gpu.index} · {gpu.name}</span>
    <span className="block text-[10px] text-text-muted">{budget?`${((budget.budget_mib-Math.max(budget.reserved_mib,held?.mib??0))/1024).toFixed(1)} GiB unreserved / ${(budget.budget_mib/1024).toFixed(1)} GiB budget · ${estimate==null?'exclusive':`needs ${(estimate/1024).toFixed(1)} GiB`}`:fit.reason}</span>
    <span className="block text-[10px] text-text-muted">{gpu.unified_memory?'Unified system memory':gpu.memory_used_mib==null||gpu.memory_total_mib==null?'VRAM measurement unavailable':`${(gpu.memory_used_mib/1024).toFixed(1)} / ${(gpu.memory_total_mib/1024).toFixed(1)} GiB VRAM used`}{gpu.utilization_percent==null?'':` · ${gpu.utilization_percent.toFixed(0)}% utilization`}</span>
    {budget?.exclusive_deployment&&<span className="block text-[10px] text-text-muted">Reserved by {budget.exclusive_deployment}</span>}
   </span>
  </label>;
 })}</fieldset>;
}
