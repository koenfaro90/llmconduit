import type {ManagementState, MeshNode, RecipeRevision} from '../../api/types';
import {deploymentRows, runtimeSpec} from './managementModel';
export const compareNames=(a:string,b:string)=>a.localeCompare(b,undefined,{numeric:true,sensitivity:'base'});
export const hostName=(n:MeshNode)=>n.host_snapshot?.hostname??n.label??n.endpoint_id.slice(0,12);
export const shortHost=(name:string)=>name.split('.')[0]??name;
export const sortHosts=(nodes:MeshNode[])=>nodes.slice().sort((a,b)=>compareNames(hostName(a),hostName(b)));
export const GPU_FAMILIES={'spark':'DGX Spark · NVIDIA GB10','rtx-pro-6000-blackwell':'RTX PRO 6000 Blackwell'};
export function gpuFamily(name:string,unified=false):string {
  if (/GB10/i.test(name)||unified) return 'spark';
  if (/RTX.*(?:PRO.*6000|6000.*PRO).*Blackwell/i.test(name)) return 'rtx-pro-6000-blackwell';
  return name||'unknown';
}
export function hostFamilies(node:MeshNode){return [...new Set(node.host_snapshot?.gpus.map(g=>gpuFamily(g.name,g.unified_memory))??[])];}
export function hardwareLabel(node:MeshNode){const families=hostFamilies(node);return families.map(f=>GPU_FAMILIES[f as keyof typeof GPU_FAMILIES]??f).join(' / ')||'GPU type unavailable';}
export function rankSpecs(spec:Record<string,unknown>):Record<string,unknown>[]{return Array.isArray(spec.distributed)?spec.distributed as Record<string,unknown>[]:[spec];}
export interface RecipeMetadata {checkpoints?:import('./preparationModel').CheckpointRequirement[];source_url?:string;size_name?:string;size_source_digest?:string;memory_estimate_note?:string;gpu_families?:string[];image_sources?:Record<string,string>;image_source_urls?:Record<string,string>}
export function metadata(spec:Record<string,unknown>):RecipeMetadata {return spec.metadata&&typeof spec.metadata==='object'?spec.metadata as RecipeMetadata:{};}
export function gpuDevices(spec:Record<string,unknown>):number[]|null {
  const value=String(spec.gpus??''); const match=value.match(/^device=([0-9,]+)$/);
  return match?[...new Set(match[1]!.split(',').map(Number))]:null;
}
export function gpuCount(spec:Record<string,unknown>):number|null {const counts=rankSpecs(spec).map(s=>gpuDevices(s)?.length??null);return counts.every(c=>c!=null)?counts.reduce<number>((a,c)=>a+c!,0):null;}
export function recipeFamilies(revision:RecipeRevision){return metadata(revision.model_spec).gpu_families??[];}
export function compatible(revision:RecipeRevision,node:MeshNode){const required=recipeFamilies(revision);return !required.length||required.some(f=>hostFamilies(node).includes(f));}
export function hostCapacity(node:MeshNode,state:ManagementState|undefined){
  const snapshot=node.host_snapshot; const age=Date.now()-Date.parse(snapshot?.sampled_at??'');const fresh=node.enabled&&Number.isFinite(age)&&age>=-30000&&age<=20000;
  const occupied=deploymentRows(state,[node]).filter(r=>r.deployment.endpoint_id===node.endpoint_id&&(r.deployment.desired_state==='ready'||r.deployment.observed?.status?.phase==='ready'||['loading','stopping','unhealthy','unknown'].includes(String(r.deployment.observed?.status?.phase))));
  const reserved=new Set<number>();let unknown=false;for(const row of occupied){const devices=gpuDevices(runtimeSpec(row));if(devices)devices.forEach(d=>reserved.add(d));else unknown=true;}
  const free=snapshot?.gpus.filter(g=>!unknown&&!reserved.has(g.index))??[];
  const dedicated=snapshot?.gpus.filter(g=>!g.unified_memory)??[];
  const known=dedicated.length>0&&dedicated.every(g=>g.memory_total_mib!=null&&g.memory_used_mib!=null);
  return {fresh,freeGpuIndices:free.map(g=>g.index),totalGpus:snapshot?.gpus.length??0,occupied:occupied.map(r=>r.model),unified:snapshot?.gpus.some(g=>g.unified_memory)??false,freeVramMiB:fresh&&known?dedicated.reduce((sum,g)=>sum+Math.max(0,g.memory_total_mib!-g.memory_used_mib!),0):null,freeRamBytes:fresh&&snapshot?.memory?snapshot.memory.available_bytes:null};
}
export function vramLabel(node:MeshNode){const gpus=node.host_snapshot?.gpus??[];if(gpus.some(g=>g.unified_memory))return 'Unified system RAM';if(!gpus.length||gpus.some(g=>g.memory_used_mib==null||g.memory_total_mib==null))return '—';return `${(gpus.reduce((a,g)=>a+g.memory_used_mib!,0)/1024).toFixed(1)} / ${(gpus.reduce((a,g)=>a+g.memory_total_mib!,0)/1024).toFixed(1)} GiB`;}
export function sampleTime(value:string|undefined){const time=Date.parse(value??'');return Number.isFinite(time)?new Date(time).toLocaleString(undefined,{year:'numeric',month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',second:'2-digit'}):'—';}
/** Catalog annotations do not change the executable configuration or invalidate its evidence. */
export function executionKey(spec:Record<string,unknown>):string {
  function withoutAnnotations(model:Record<string,unknown>):Record<string,unknown>{const copy={...model};delete copy.metadata;delete copy.sizes;if(Array.isArray(copy.distributed))copy.distributed=copy.distributed.map(rank=>withoutAnnotations(rank as Record<string,unknown>));return copy;}
  function canonical(value:unknown):unknown {if(Array.isArray(value))return value.map(canonical);if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).sort(([a],[b])=>compareNames(a,b)).map(([key,item])=>[key,canonical(item)]));return value;}
  return JSON.stringify(canonical(withoutAnnotations(spec)));
}
