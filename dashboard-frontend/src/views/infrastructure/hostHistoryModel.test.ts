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

it('retains per-GPU fan, clocks, temperature, utilization and PCIe history by physical UUID',()=>{
 const host=sample(1000);host.gpus[0]!.fan_speed_percent=0;host.gpus[0]!.memory_clock_mhz=12000;host.gpus[0]!.pcie_rx_kb_per_second=1000;
 host.gpus.push({...host.gpus[0]!,index:1,uuid:'second',utilization_percent:70,temperature_celsius:55,clock_mhz:1800,fan_speed_percent:123,pcie_rx_kb_per_second:5000});
 expect(historyPoints([host],'gpu',15000,'second')).toEqual([[1000,70]]);
 expect(historyPoints([host],'clock',15000,'second')).toEqual([[1000,1800]]);
 expect(historyPoints([host],'memory_clock',15000,'second')).toEqual([[1000,12000]]);
 expect(historyPoints([host],'temperature',15000,'second')).toEqual([[1000,55]]);
 expect(historyPoints([host],'pcie_rx',15000,'second')).toEqual([[1000,5]]);
 expect(historyPoints([host],'fan',15000,'second')).toEqual([[1000,123]]);
 expect(historyPoints([host],'fan',15000,'gpu')).toEqual([[1000,0]]);
 expect(historyPoints([host],'fan',15000,'missing')).toEqual([[1000,null]]);
});
it('leaves unsupported fan speeds unavailable and honors fan off timestamps',()=>{
 const host=sample(1000);expect(HOST_METRICS.fan.read(host)).toBeNull();
 host.gpus[0]!.fan_speed_percent=50;host.collection={host_interval_ms:5000,gpu_interval_ms:5000,container_interval_ms:5000,gpu_backend:'nvml',gpu_sampled_at:null,containers_sampled_at:null,gpu_duration_ms:null,metrics:{gpu_fan:{interval_ms:0,sampled_at:null}}};
 expect(HOST_METRICS.fan.read(host)).toBeNull();
});

const filesystemRow={mount_point:'/',mount_points:['/'],device:'/dev/sda1',device_id:'8:1',filesystem_type:'ext4',read_only:false,total_bytes:1000,free_bytes:600,available_bytes:500,used_bytes:400,inodes_total:100,inodes_free:30,inodes_used:70};
const blockRow={name:'sda',device_id:'8:0',leaf_device:true,read_bytes_per_second:1024**2,write_bytes_per_second:0,read_iops:10,write_iops:0,read_latency_ms:2,write_latency_ms:null,busy_percent:20,in_flight:2,average_queue_depth:1};
it('uses root capacity and leaf I/O totals without double counting logical layers',()=>{
 const host=sample(1000);host.filesystems=[filesystemRow,{...filesystemRow,mount_point:'/data',mount_points:['/data'],device_id:'8:2',used_bytes:900,inodes_total:null,inodes_free:null,inodes_used:null}];
 host.block_devices=[blockRow,{...blockRow,name:'dm-0',device_id:'253:0',leaf_device:false,read_bytes_per_second:99*1024**2}];
 expect(HOST_METRICS.filesystem_usage.read(host)).toBe(40);expect(HOST_METRICS.inode_usage.read(host)).toBe(70);
 expect(HOST_METRICS.disk_read.read(host)).toBe(1);expect(HOST_METRICS.disk_busy.read(host)).toBe(20);
 expect(HOST_METRICS.disk_read_latency.read(host)).toBe(2);expect(HOST_METRICS.disk_write_latency.read(host)).toBeNull();
 expect(historyPoints([host],'filesystem_usage',15000,undefined,{filesystem:'/data'})).toEqual([[1000,90]]);
 expect(historyPoints([host],'inode_usage',15000,undefined,{filesystem:'/data'})).toEqual([[1000,null]]);
 expect(historyPoints([host],'disk_read',15000,undefined,{device:'253:0'})).toEqual([[1000,99]]);
 expect(historyPoints([host],'disk_read',15000,undefined,{device:'missing'})).toEqual([[1000,null]]);
 host.block_devices.push({...blockRow,name:'sdb',device_id:'8:16',read_iops:30,read_latency_ms:6});
 expect(HOST_METRICS.disk_read_latency.read(host)).toBe(5);
 host.block_devices[0]!.read_bytes_per_second=null;expect(HOST_METRICS.disk_read.read(host)).toBeNull();
});
it('honors storage off/stale metadata and does not substitute another mount for root',()=>{
 const host=sample(121000);host.filesystems=[{...filesystemRow,mount_point:'/data',mount_points:['/data']}];host.block_devices=[blockRow];
 expect(HOST_METRICS.filesystem_usage.read(host)).toBeNull();
 host.collection={host_interval_ms:1000,gpu_interval_ms:5000,container_interval_ms:5000,gpu_backend:'nvml',gpu_sampled_at:null,containers_sampled_at:null,gpu_duration_ms:null,metrics:{disk:{interval_ms:0,sampled_at:null},disk_io:{interval_ms:1000,sampled_at:new Date(1000).toISOString()}}};
 expect(historyPoints([host],'filesystem_usage',15000,undefined,{filesystem:'/data'})).toEqual([[121000,null]]);
 expect(HOST_METRICS.disk_read.read(host)).toBeNull();
});
