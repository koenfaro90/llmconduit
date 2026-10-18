export interface CachedImage {id:string;tags:string[]|null;digests:string[]|null;architecture:string;os:string;size_bytes:number}
export interface CachedSnapshot {repository:string;revision:string;top_level_entries:number;top_level_logical_bytes:number;subfolders:string[];truncated:boolean;unreadable_entries:number}
export interface ArtifactInventory {sampled_at:string;images:CachedImage[];snapshots:CachedSnapshot[];cache_configured:boolean;presence_only:true;truncated:boolean;errors:string[]}
const object=(v:unknown):v is Record<string,unknown>=>typeof v==='object'&&v!==null&&!Array.isArray(v);
const strings=(v:unknown)=>Array.isArray(v)&&v.every(s=>typeof s==='string'&&s.length<=512);
const count=(v:unknown)=>typeof v==='number'&&Number.isSafeInteger(v)&&v>=0;
export function isArtifactInventory(v:unknown):v is ArtifactInventory {
 return object(v)&&typeof v.sampled_at==='string'&&Number.isFinite(Date.parse(v.sampled_at))&&v.presence_only===true&&typeof v.cache_configured==='boolean'&&typeof v.truncated==='boolean'&&strings(v.errors)&&Array.isArray(v.images)&&v.images.length<=128&&v.images.every(r=>object(r)&&typeof r.id==='string'&&/^sha256:[a-f0-9]{64}$/.test(r.id)&&(r.tags===null||strings(r.tags))&&(r.digests===null||strings(r.digests))&&typeof r.architecture==='string'&&typeof r.os==='string'&&count(r.size_bytes))&&Array.isArray(v.snapshots)&&v.snapshots.length<=128&&v.snapshots.every(r=>object(r)&&typeof r.repository==='string'&&typeof r.revision==='string'&&/^[a-f0-9]{40}$/.test(r.revision)&&count(r.top_level_entries)&&count(r.top_level_logical_bytes)&&strings(r.subfolders)&&typeof r.truncated==='boolean'&&count(r.unreadable_entries));
}
