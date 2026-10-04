import type {MeshNode,RecipeRevision,ManagementState} from '../../api/types';
import {hostCapacity,gpuDevices} from './hardwareModel';
import {placedSpec} from './managementModel';
export interface SizeDefinition {description?:string;overrides?:Record<string,unknown>;vllm?:Record<string,string|number>}
export function sizes(revision:RecipeRevision):Record<string,SizeDefinition>{return revision.model_spec.sizes as Record<string,SizeDefinition>??{};}
export function sizeName(revision:RecipeRevision|undefined){return (revision?.model_spec.metadata as Record<string,unknown>|undefined)?.size_name as string|undefined;}
export function sizeSpec(revision:RecipeRevision,name:string):Record<string,unknown>{
 const spec=structuredClone(revision.model_spec);const definition=sizes(revision)[name];delete spec.sizes;
 function apply(rank:Record<string,unknown>){Object.assign(rank,definition?.overrides??{});if(definition?.vllm?.gpu_memory_utilization!=null&&rank.gpu_memory&&(definition.overrides?.gpu_memory as {fraction?:number}|undefined)?.fraction==null){rank.gpu_memory={...rank.gpu_memory as object,fraction:Number(definition.vllm.gpu_memory_utilization)};}return rank;}
 if(Array.isArray(spec.distributed))spec.distributed=spec.distributed.map(rank=>apply(rank as Record<string,unknown>));else apply(spec);
 return spec;
}
export function memoryMiB(raw:unknown):number|null{const match=String(raw??'').match(/^([1-9][0-9]*)([mMgG])$/);return match?Number(match[1])*(match[2]!.toLowerCase()==='g'?1024:1):null;}
export function gpuEstimate(spec:Record<string,unknown>,totalMiB:number|null){
 const budget=spec.gpu_memory as {mode?:string;mib_per_device?:number;fraction?:number}|undefined;
 if(!budget)return null;const fraction=budget.fraction??0;return fraction&&totalMiB==null?null:Math.max(budget.mib_per_device??0,Math.ceil(fraction*(totalMiB??0)));
}
export function configuredUtilization(spec:Record<string,unknown>):number|null {const env=spec.environment as Record<string,unknown>|undefined;const command=Array.isArray(spec.command)?spec.command.map(String).join(' '):'';const raw=command.match(/--gpu-memory-utilization(?:=|\s+)([0-9.]+)/)?.[1]??env?.GPU_MEMORY_UTILIZATION;const value=Number(raw);return raw!=null&&Number.isFinite(value)&&value>0&&value<=1?value:null;}
export function resourceLabel(spec:Record<string,unknown>){const ram=memoryMiB(spec.memory_limit);const budget=spec.gpu_memory as {mode?:string;mib_per_device?:number;fraction?:number}|undefined;return `${ram==null?'—':`${(ram/1024).toFixed(1)} GiB RAM`} · ${budget?.mode==='shared'?`shared GPU: max(${((budget.mib_per_device??0)/1024).toFixed(1)} GiB, ${((budget.fraction??0)*100).toFixed(0)}%)`:`exclusive GPU${configuredUtilization(spec)==null?'':` · configured target ${(configuredUtilization(spec)!*100).toFixed(0)}%`}`}`;}
export function placementFit(spec:Record<string,unknown>,node:MeshNode,state:ManagementState|undefined){
 const capacity=hostCapacity(node,state);if(!capacity.fresh)return {fits:false,reason:'Capacity unavailable · stale sample'};
 const required=gpuDevices(spec);const shared=(spec.gpu_memory as {mode?:string}|undefined)?.mode==='shared';const policy=node.host_snapshot?.managed_capacity;
 if(shared&&(!policy||!policy.gpu_sharing_enabled))return {fits:false,reason:'Shared GPU access is not enabled on this host'};
 if(!policy)return {fits:required?required.every(index=>capacity.freeGpuIndices.includes(index)):capacity.freeGpuIndices.length>0,reason:'Exclusive GPU reservation'};
 const ledger=resourceReservations(node,state);const ram=ledger.ram;const reservation=ledger.gpus;
 const requestedRam=memoryMiB(spec.memory_limit);if(requestedRam==null||Math.max(policy.memory_reserved_mib,ram)+requestedRam>policy.memory_budget_mib)return {fits:false,reason:'Insufficient unreserved RAM budget'};
 if(!required?.length)return {fits:true,reason:'RAM budget available'};
 let unified=0;
 for(const index of required){const gpu=policy.gpus.find(g=>g.index===index);if(!gpu)return {fits:false,reason:`GPU ${index} capacity unavailable`};const held=reservation.get(index);if(gpu.exclusive_deployment||held?.exclusive||!shared&&(gpu.reserved_mib>0||(held?.mib??0)>0))return {fits:false,reason:`GPU ${index} reserved exclusively`};const amount=shared?gpuEstimate(spec,gpu.total_mib):gpu.budget_mib;if(amount==null||Math.max(gpu.reserved_mib,held?.mib??0)+amount>gpu.budget_mib)return {fits:false,reason:`Insufficient GPU ${index} memory budget`};if(shared&&gpu.unified_memory)unified+=amount;}
 if(unified>requestedRam)return {fits:false,reason:'Unified GPU estimate exceeds RAM budget'};
 return {fits:true,reason:shared?'Shared GPU and RAM budgets available':'Exclusive GPU and RAM budgets available'};
}
export function defaultGpuSelection(spec:Record<string,unknown>,node:MeshNode,state:ManagementState|undefined):number[]|undefined {
 const required=gpuDevices(spec);if(!required)return undefined;
 const candidates=(node.host_snapshot?.gpus??[]).slice().sort((a,b)=>a.index-b.index).filter(g=>placementFit({...spec,gpus:`device=${g.index}`},node,state).fits).map(g=>g.index);
 return candidates.length>=required.length?candidates.slice(0,required.length):required;
}
export function resourceReservations(node:MeshNode,state:ManagementState|undefined){
 const policy=node.host_snapshot?.managed_capacity;
 let ram=0;const reservation=new Map<number,{mib:number;exclusive:boolean}>();
 for(const d of Object.values(state?.deployments??{})){
  if(d.endpoint_id!==node.endpoint_id||d.desired_state!=='ready'&&(!d.observed?.status?.phase||d.observed.status.phase==='unloaded'))continue;
  const revision=state?.recipes[d.recipe_id]?.find(r=>r.digest===d.revision_digest);const rank=placedSpec(revision?.model_spec??{},d);ram+=memoryMiB(rank.memory_limit)??policy?.memory_budget_mib??0;
  for(const index of gpuDevices(rank)??[]){const gpu=policy?.gpus.find(g=>g.index===index);const exclusive=(rank.gpu_memory as {mode?:string}|undefined)?.mode!=='shared';const old=reservation.get(index)??{mib:0,exclusive:false};reservation.set(index,{mib:old.mib+(exclusive?gpu?.budget_mib??0:gpuEstimate(rank,gpu?.total_mib??null)??gpu?.budget_mib??0),exclusive:old.exclusive||exclusive});}
 }
 return {ram:Math.max(ram,policy?.memory_reserved_mib??0),gpus:reservation};
}
