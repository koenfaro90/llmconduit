export type TrafficMode = 'off' | 'observe' | 'enforce';
export interface TrafficLimits { priority: number | null; max_concurrent: number | null; models: Record<string, number>; max_wait_ms: number | null }
export interface Schedule { timezone: string; weekdays: number[]; dates: string[]; start: string; end: string; limits: TrafficLimits; shared_concurrent: number | null; shared_models: Record<string, number> }
export interface TrafficRule extends TrafficLimits { shared_concurrent: number | null; shared_models: Record<string, number>; schedules: Schedule[] }
export interface BackendPolicy { max_concurrent: number | null; paused: boolean; resource: string | null; deployment_id: string | null }
export interface ModelPolicy { budget: string | null; replicas: string[]; balance: boolean; affinity: 'off' | 'prefer' | 'strict'; affinity_ttl_secs: number; affinity_wait_ms: number }
export interface TrafficConfig { mode: TrafficMode; defaults: TrafficRule; groups: Record<string, TrafficRule>; group_members: Record<string,string[]>; users: Record<string, TrafficRule>; keys: Record<string, TrafficRule>; models: Record<string, ModelPolicy>; backends: Record<string, BackendPolicy>; queue: { enabled: boolean; max_requests: number; max_bytes: number; per_key: number; max_wait_ms: number; aging_ms: number }; mobile_compact: boolean; capture_engine_metrics: boolean; cancellation_grace_ms:number }
export interface TrafficRequest { id: string; key: string; user: string; groups: string[]; model: string; budget: string; backend: string; priority: number; created_at_ms: number; state: string; reason: string | null; bytes: number; affinity: string }
export interface EngineMetrics {time_to_first_token_ms:number|null;generation_time_ms:number|null;queue_time_ms:number|null;mean_itl_ms:number|null;tokens_per_second:number|null}
export interface TrafficRuntime {completed_metrics:Array<{request:TrafficRequest;received_at_ms:number;metrics:EngineMetrics}>; mode: TrafficMode; ready: boolean; revision: number; queued: TrafficRequest[]; active: TrafficRequest[]; clients: number; admitted: number; rejected: number; cancelled: number; shadow_blocked: number; queued_bytes: number; reservations: Record<string, string> }
export interface TrafficResponse { revision: number; config: TrafficConfig; runtime: TrafficRuntime; scope: string }
export interface Backend { provider_id: string; provider_name: string; base_url: string; models: Array<{ id: string; context_limit?: number | null }>; healthy: boolean; accepting_requests: boolean }
export interface EngineSample { backend: string; scraped_at_ms: number; engine: string; models: Record<string, { values: Record<string, number>; by_label?: Record<string, Record<string, number>> }> }
export const blankLimits = (): TrafficLimits => ({ priority: null, max_concurrent: null, models: {}, max_wait_ms: null });
export const blankRule = (): TrafficRule => ({ ...blankLimits(), shared_concurrent: null, shared_models: {}, schedules: [] });
export const blankBackend = (): BackendPolicy => ({ max_concurrent: null, paused: false, resource: null, deployment_id: null });
export const blankModel = (): ModelPolicy => ({ budget: null, replicas: [], balance: false, affinity: 'off', affinity_ttl_secs: 1800, affinity_wait_ms: 1000 });
export function ruleSummary(rule: TrafficRule): string { return [`${rule.max_concurrent ?? 'Unbounded'} per key`, rule.shared_concurrent == null ? '' : `${rule.shared_concurrent} shared`, rule.priority == null ? '' : `weight ${rule.priority}`, rule.schedules.length ? `${rule.schedules.length} scheduled windows` : ''].filter(Boolean).join(' · '); }
export function backendRows(runtime: TrafficRuntime, config: TrafficConfig, inventory: Backend[]) {
  const urls = [...new Set([...Object.keys(config.backends), ...inventory.map(b => b.base_url.replace(/\/$/, '')), ...runtime.active.map(r => r.backend), ...runtime.queued.map(r => r.backend)])];
  return urls.map(url => {
    const policy = config.backends[url]; const physical = policy?.resource ?? url;
    const related = Object.entries(config.backends).filter(([u,p]) => (p.resource ?? u) === physical).map(([,p])=>p);
    const caps = related.flatMap(p=>p.max_concurrent==null?[]:[p.max_concurrent]);
    const matches = (r: TrafficRequest) => (config.backends[r.backend]?.resource ?? r.backend) === physical;
    return { url, resource: physical, limit: caps.length ? Math.min(...caps) : undefined, paused: related.some(p=>p.paused), active: runtime.active.filter(matches).length, queued: runtime.queued.filter(matches).length, reserved: Object.values(runtime.reservations).some(r => r === physical || r === '*') };
  }).sort((a, b) => b.queued - a.queued || b.active - a.active || a.url.localeCompare(b.url));
}
export function isTrafficResponse(value: unknown): value is TrafficResponse {
  if (!value || typeof value !== 'object') return false;
  const v = value as TrafficResponse;
  return Number.isInteger(v.revision) && !!v.config && ['off', 'observe', 'enforce'].includes(v.config.mode) && !!v.runtime && Array.isArray(v.runtime.active) && Array.isArray(v.runtime.queued) && v.runtime.active.concat(v.runtime.queued).every(r => r && typeof r.id === 'string' && typeof r.model === 'string' && typeof r.backend === 'string' && Number.isFinite(r.created_at_ms));
}

export const blankConfig=():TrafficConfig=>({mode:'off',defaults:blankRule(),groups:{},group_members:{},users:{},keys:{},models:{},backends:{},queue:{enabled:false,max_requests:512,max_bytes:32*1024**2,per_key:32,max_wait_ms:30000,aging_ms:10000},mobile_compact:false,capture_engine_metrics:false,cancellation_grace_ms:0});
export const blankRuntime=():TrafficRuntime=>({completed_metrics:[],mode:'off',ready:true,revision:0,queued:[],active:[],clients:0,admitted:0,rejected:0,cancelled:0,shadow_blocked:0,queued_bytes:0,reservations:{}});
