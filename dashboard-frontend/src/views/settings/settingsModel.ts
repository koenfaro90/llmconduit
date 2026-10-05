export type Json = null | boolean | number | string | Json[] | {[key:string]:Json};
export interface SettingsView {revision:number;applied_revision:number;status:string;error:string|null;values:Record<string,Json>;active_values:Record<string,Json>;environment:Record<string,string>;impact:string;source:string;available_environment?:string[];effective_values?:Record<string,Json>}
export interface HostSettings {revision:number;applied_revision:number;enabled:boolean;configuration:{metrics:Record<string,string>;policy:Record<string,Json>}}
export const SECRET='[keep existing secret]';
const record=(v:unknown):v is Record<string,unknown>=>typeof v==='object'&&v!==null&&!Array.isArray(v);
const integer=(v:unknown)=>typeof v==='number'&&Number.isSafeInteger(v)&&v>=0;
export function isSettingsView(v:unknown):v is SettingsView {return record(v)&&integer(v.revision)&&integer(v.applied_revision)&&typeof v.status==='string'&&(v.error===null||typeof v.error==='string')&&record(v.values)&&record(v.active_values)&&record(v.environment)&&Object.values(v.environment).every(x=>typeof x==='string')&&typeof v.impact==='string'&&typeof v.source==='string'}
export function isHostSettings(v:unknown):v is HostSettings {return record(v)&&integer(v.revision)&&integer(v.applied_revision)&&typeof v.enabled==='boolean'&&record(v.configuration)&&record(v.configuration.metrics)&&Object.values(v.configuration.metrics).every(x=>typeof x==='string')&&record(v.configuration.policy)}
export function changedSettings(a:Record<string,Json>,b:Record<string,Json>):string[]{return [...new Set([...Object.keys(a),...Object.keys(b)])].filter(k=>JSON.stringify(a[k])!==JSON.stringify(b[k]))}
export function title(path:string):string{return path.split('/').filter(Boolean).at(-1)!.replaceAll('_',' ')}

export function secretName(name:string):boolean{return /(?:password|secret|api_key|pepper|session_key)|(?:^|_)token$|(?:^|_)key$/i.test(name)&&!name.endsWith("_env")}

export function overridesFor(path:string):string[]{
 const specific:Record<string,string[]>={'/control_plane/storage/backend':['LLMCONDUIT_STORAGE_BACKEND'],'/control_plane/storage/url':['LLMCONDUIT_DATABASE_URL'],'/control_plane/storage/queue_capacity':['LLMCONDUIT_PERSISTENCE_QUEUE_CAPACITY'],'/control_plane/storage/retention_days':['LLMCONDUIT_PERSISTENCE_RETENTION_DAYS'],'/control_plane/auth':['LLMCONDUIT_REQUIRE_AUTH','LLMCONDUIT_CONVERSATION_ID_HEADER'],'/brave_api_key':['BRAVE_SEARCH_API_KEY'],'/upstream_api_key':['LLMCONDUIT_UPSTREAM_API_KEY','OPENAI_API_KEY'],'/upstream_chat_kwargs':['LLMCONDUIT_UPSTREAM_CHAT_KWARGS_JSON'],'/price_table':['LLMCONDUIT_PRICE_TABLE_JSON']};
 return specific[path]??(path.split('/').length===2?['LLMCONDUIT_'+path.slice(1).toUpperCase()]:[]);
}
