import {expect,it} from 'vitest';
import type {FleetHostSnapshot} from '../../api/types';
import {HOST_METRICS,historyPoints} from './hostHistoryModel';
const sample=(at:number):FleetHostSnapshot=>({schema_version:1,sampled_at:new Date(at).toISOString(),hostname:'spark',architecture:'arm64',uptime_seconds:1,cpu:null,memory:{total_bytes:100,used_bytes:40,available_bytes:60},disk:null,network:[],containers:[],errors:[],gpus:[{index:0,uuid:'gpu',name:'GB10',unified_memory:true,memory_total_mib:null,memory_used_mib:null,utilization_percent:20,temperature_celsius:40,power_watts:null,clock_mhz:1200}]});
it('keeps GB10 dedicated memory unavailable and preserves gaps in stored history',()=>{const a=sample(1000),b=sample(121000);expect(HOST_METRICS.gpu_memory.read(a)).toBeNull();expect(HOST_METRICS.ram.read(a)).toBe(40);expect(HOST_METRICS.clock.read(a)).toBe(1200);expect(historyPoints([a,b],'gpu',15000)).toEqual([[1000,20],[16000,null],[121000,20]]);});

it('plots GPU PCIe totals without manufacturing missing device measurements',()=>{
 const host=sample(1000);
 expect(HOST_METRICS.pcie_rx.read(host)).toBeNull();
 host.gpus[0]!.pcie_rx_kb_per_second=1000000;host.gpus[0]!.pcie_tx_kb_per_second=0;
 expect(HOST_METRICS.pcie_rx.read(host)).toBe(1000);expect(HOST_METRICS.pcie_tx.read(host)).toBe(0);
 host.gpus.push({...host.gpus[0]!,index:1,uuid:'second',pcie_rx_kb_per_second:undefined});
 expect(HOST_METRICS.pcie_rx.read(host)).toBeNull();
 host.gpus[1]!.pcie_rx_kb_per_second=500000;expect(HOST_METRICS.pcie_rx.read(host)).toBe(1500);
});

it('honors individual disabled and stale metric timestamps in stored history',()=>{
 const host=sample(121000);host.collection={host_interval_ms:100,gpu_interval_ms:100,container_interval_ms:5000,gpu_backend:'nvml',gpu_sampled_at:null,containers_sampled_at:null,gpu_duration_ms:null,metrics:{gpu_clock:{interval_ms:0,sampled_at:null},memory:{interval_ms:60000,sampled_at:new Date(1000).toISOString()},gpu_utilization:{interval_ms:100,sampled_at:new Date(1000).toISOString()}}};
 expect(HOST_METRICS.clock.read(host)).toBeNull();expect(HOST_METRICS.ram.read(host)).toBe(40);expect(HOST_METRICS.gpu.read(host)).toBeNull();
 host.collection.metrics!.memory!.sampled_at='bad';expect(HOST_METRICS.ram.read(host)).toBeNull();
});
