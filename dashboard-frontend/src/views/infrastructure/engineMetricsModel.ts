import type {ChartSeries} from '../../viz/LineChart';
export interface EngineMetric {name:string;family:string;type:string;help:string;labels:Record<string,string>;value:number|null;special:string}
export interface EngineMetricSample {seq:number;deployment_id:string;generation:number;recipe_digest:string;image:string;sampled_at_ms:number;duration_ms:number;metrics:EngineMetric[];error:string}
export interface EngineMetricHistory {samples:EngineMetricSample[];next_until_ms:number|null;resolution_ms:number;includes_direct_traffic:boolean}
export function metricKey(m:EngineMetric):string{return `${m.name}${JSON.stringify(Object.entries(m.labels).sort(([a],[b])=>a.localeCompare(b)))}`}
export function metricLabel(m:EngineMetric):string {return `${m.name.replace(/^vllm:/,'')} ${Object.entries(m.labels).map(([k,v])=>`${k}=${v}`).join(', ')}`.trim()}
export function engineSeries(samples:EngineMetricSample[],selected:string[],rates:boolean):ChartSeries[]{
 const ordered=[...new Map(samples.map(s=>[s.sampled_at_ms,s])).values()].sort((a,b)=>a.sampled_at_ms-b.sampled_at_ms);
 const metrics=new Map(ordered.flatMap(s=>s.metrics).map(m=>[metricKey(m),m]));
 return selected.flatMap(key=>{const metric=metrics.get(key);if(!metric)return [];let previous:{at:number,value:number}|null=null;const points:Array<[number,number|null]>=[];
 const counter=metric.type==='counter'||(['histogram','summary'].includes(metric.type)&&/_(bucket|count|sum)$/.test(metric.name));
 for(const sample of ordered){const value=sample.metrics.find(m=>metricKey(m)===key)?.value??null;
 if(previous&&sample.sampled_at_ms-previous.at>1500){points.push([previous.at+1000,null]);previous=null}
 let y=value;if(rates&&counter){y=previous&&value!=null&&value>=previous.value?(value-previous.value)/((sample.sampled_at_ms-previous.at)/1000):null}
 points.push([sample.sampled_at_ms,y]);previous=value==null?null:{at:sample.sampled_at_ms,value};}
 return [{key,label:metricLabel(metric)+(rates&&counter?' /s':''),points}];});
}
