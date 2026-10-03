import {expect,it} from 'vitest';
import type {FleetHostSnapshot} from '../../api/types';
import {HOST_METRICS,historyPoints} from './hostHistoryModel';
const sample=(at:number):FleetHostSnapshot=>({schema_version:1,sampled_at:new Date(at).toISOString(),hostname:'spark',architecture:'arm64',uptime_seconds:1,cpu:null,memory:{total_bytes:100,used_bytes:40,available_bytes:60},disk:null,network:[],containers:[],errors:[],gpus:[{index:0,uuid:'gpu',name:'GB10',unified_memory:true,memory_total_mib:null,memory_used_mib:null,utilization_percent:20,temperature_celsius:40,power_watts:null,clock_mhz:1200}]});
it('keeps GB10 dedicated memory unavailable and preserves gaps in stored history',()=>{const a=sample(1000),b=sample(121000);expect(HOST_METRICS.gpu_memory.read(a)).toBeNull();expect(HOST_METRICS.ram.read(a)).toBe(40);expect(HOST_METRICS.clock.read(a)).toBe(1200);expect(historyPoints([a,b],'gpu',15000)).toEqual([[1000,20],[16000,null],[121000,20]]);});
