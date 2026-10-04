import type {FleetHostSnapshot} from '../../api/types';
export const FILESYSTEM_METRIC_IDS=['filesystem_usage','inode_usage'] as const;
export const BLOCK_DEVICE_METRIC_IDS=['disk_read','disk_write','disk_read_iops','disk_write_iops','disk_read_latency','disk_write_latency','disk_busy','disk_in_flight','disk_queue'] as const;
function filesystem(h:FleetHostSnapshot) {return h.filesystems?.find(f=>f.mount_point==='/');}
function diskSum(h:FleetHostSnapshot,key:keyof NonNullable<FleetHostSnapshot['block_devices']>[number],scale=1) {
 const values=h.block_devices?.filter(d=>d.leaf_device).map(d=>d[key]);
 return values?.length&&values.every((v):v is number=>typeof v==='number')?values.reduce((a,b)=>a+b,0)/scale:null;
}
function diskLatency(h:FleetHostSnapshot,direction:'read'|'write') {
 const devices=h.block_devices?.filter(d=>d.leaf_device);
 if(!devices?.length||devices.some(d=>d[`${direction}_iops`]==null))return null;
 const active=devices.filter(d=>(d[`${direction}_iops`]??0)>0);
 if(!active.length||active.some(d=>d[`${direction}_latency_ms`]==null))return null;
 const total=active.reduce((sum,d)=>sum+d[`${direction}_iops`]!,0);
 return active.reduce((sum,d)=>sum+d[`${direction}_latency_ms`]!*d[`${direction}_iops`]!,0)/total;
}
const STORAGE_METRICS={
 filesystem_usage:{label:'Filesystem usage',unit:'%',read:(h:FleetHostSnapshot)=>{const f=filesystem(h)??h.disk;return f?.total_bytes?f.used_bytes/f.total_bytes*100:null;}},
 inode_usage:{label:'Inode usage',unit:'%',read:(h:FleetHostSnapshot)=>{const f=filesystem(h);return f?.inodes_total&&f.inodes_used!=null?f.inodes_used/f.inodes_total*100:null;}},
 disk_read:{label:'Disk read',unit:' MiB/s',read:(h:FleetHostSnapshot)=>diskSum(h,'read_bytes_per_second',1024**2)},
 disk_write:{label:'Disk write',unit:' MiB/s',read:(h:FleetHostSnapshot)=>diskSum(h,'write_bytes_per_second',1024**2)},
 disk_read_iops:{label:'Disk read IOPS',unit:' IOPS',read:(h:FleetHostSnapshot)=>diskSum(h,'read_iops')},
 disk_write_iops:{label:'Disk write IOPS',unit:' IOPS',read:(h:FleetHostSnapshot)=>diskSum(h,'write_iops')},
 disk_read_latency:{label:'Disk read latency',unit:' ms',read:(h:FleetHostSnapshot)=>diskLatency(h,'read')},
 disk_write_latency:{label:'Disk write latency',unit:' ms',read:(h:FleetHostSnapshot)=>diskLatency(h,'write')},
 disk_busy:{label:'Disk busy',unit:'%',read:(h:FleetHostSnapshot)=>{const values=h.block_devices?.filter(d=>d.leaf_device).map(d=>d.busy_percent);return values?.length&&values.every((v):v is number=>v!=null)?Math.max(...values):null;}},
 disk_in_flight:{label:'Disk in-flight I/O',unit:' requests',read:(h:FleetHostSnapshot)=>diskSum(h,'in_flight')},
 disk_queue:{label:'Disk average queue depth',unit:' requests',read:(h:FleetHostSnapshot)=>diskSum(h,'average_queue_depth')},
};
const RAW_HOST_METRICS={...STORAGE_METRICS,cpu:{label:'CPU utilization',unit:'%',read:(h:FleetHostSnapshot)=>h.cpu?.utilization_percent??null},ram:{label:'System memory',unit:'%',read:(h:FleetHostSnapshot)=>h.memory?.total_bytes?h.memory.used_bytes/h.memory.total_bytes*100:null},gpu:{label:'GPU utilization',unit:'%',read:(h:FleetHostSnapshot)=>mean(h.gpus.map(g=>g.utilization_percent))},gpu_memory:{label:'GPU memory',unit:'%',read:(h:FleetHostSnapshot)=>mean(h.gpus.map(g=>g.memory_total_mib&&g.memory_used_mib!=null?g.memory_used_mib/g.memory_total_mib*100:null))},rx:{label:'Network RX',unit:' MiB/s',read:(h:FleetHostSnapshot)=>network(h,'rx_bytes_per_second')},tx:{label:'Network TX',unit:' MiB/s',read:(h:FleetHostSnapshot)=>network(h,'tx_bytes_per_second')},clock:{label:'GPU SM clock',unit:' MHz',read:(h:FleetHostSnapshot)=>mean(h.gpus.map(g=>g.clock_mhz??null))},memory_clock:{label:'GPU memory clock',unit:' MHz',read:(h:FleetHostSnapshot)=>mean(h.gpus.map(g=>g.memory_clock_mhz??null))},pcie_rx:{label:'GPU PCIe RX',unit:' MB/s',read:(h:FleetHostSnapshot)=>sumGPU(h,'pcie_rx_kb_per_second',1000)},pcie_tx:{label:'GPU PCIe TX',unit:' MB/s',read:(h:FleetHostSnapshot)=>sumGPU(h,'pcie_tx_kb_per_second',1000)},fan:{label:'GPU fan speed',unit:'%',read:(h:FleetHostSnapshot)=>mean(h.gpus.map(g=>g.fan_speed_percent??null))},temperature:{label:'GPU temperature',unit:' °C',read:(h:FleetHostSnapshot)=>mean(h.gpus.map(g=>g.temperature_celsius))}};
const METRIC_NAMES:Record<keyof typeof RAW_HOST_METRICS,string>={filesystem_usage:'disk',inode_usage:'disk',disk_read:'disk_io',disk_write:'disk_io',disk_read_iops:'disk_io',disk_write_iops:'disk_io',disk_read_latency:'disk_io',disk_write_latency:'disk_io',disk_busy:'disk_io',disk_in_flight:'disk_io',disk_queue:'disk_io',cpu:'cpu',ram:'memory',gpu:'gpu_utilization',gpu_memory:'gpu_memory',rx:'network',tx:'network',clock:'gpu_clock',memory_clock:'gpu_memory_clock',pcie_rx:'pcie_rx',pcie_tx:'pcie_tx',temperature:'gpu_temperature',fan:'gpu_fan'};
export const HOST_METRICS=Object.fromEntries(Object.entries(RAW_HOST_METRICS).map(([key,value])=>[key,{...value,read:(host:FleetHostSnapshot)=>{
 const metadata=host.collection?.metrics?.[METRIC_NAMES[key as keyof typeof RAW_HOST_METRICS]];
 if(metadata){const age=Date.parse(host.sampled_at)-Date.parse(metadata.sampled_at??'');if(metadata.interval_ms===0||!Number.isFinite(age)||age < -30_000||age>Math.max(15_000,3*metadata.interval_ms))return null;}
 return value.read(host);
}}])) as typeof RAW_HOST_METRICS;
export type HostMetric=keyof typeof HOST_METRICS;
export const GPU_METRIC_IDS:readonly HostMetric[]=['gpu','gpu_memory','clock','memory_clock','pcie_rx','pcie_tx','temperature','fan'];
function mean(values:(number|null)[]) {const valid=values.filter((v):v is number=>v!=null);return valid.length?valid.reduce((a,b)=>a+b,0)/valid.length:null;}
function network(h:FleetHostSnapshot,key:'rx_bytes_per_second'|'tx_bytes_per_second') {const values=h.network.filter(n=>n.name!=='lo').map(n=>n[key]);const valid=values.filter((v):v is number=>v!=null);return valid.length?valid.reduce((a,b)=>a+b,0)/1024**2:null;}
export const HISTORY_RANGES=[{id:'hour',label:'Past hour',ms:3600_000},{id:'8hours',label:'Past 8 hours',ms:8*3600_000},{id:'day',label:'Past 24 hours',ms:24*3600_000},{id:'week',label:'Past week',ms:7*86400_000},{id:'month',label:'Past month',ms:31*86400_000}];
export function historyPoints(samples:FleetHostSnapshot[],metric:HostMetric,bucketMs:number,gpuUUID?:string,storage?:{filesystem?:string;device?:string}):Array<[number,number|null]> {const points:Array<[number,number|null]>=[];let last:number|undefined;for(const sample of samples){const time=Date.parse(sample.sampled_at);if(!Number.isFinite(time))continue;if(last!=null&&time-last>Math.max(45_000,bucketMs*2))points.push([last+bucketMs,null]);points.push([time,HOST_METRICS[metric].read(storage?.filesystem?{...sample,disk:null,filesystems:sample.filesystems?.filter(f=>f.mount_point===storage.filesystem).map(f=>({...f,mount_point:'/'}))}:storage?.device?{...sample,block_devices:sample.block_devices?.filter(d=>d.device_id===storage.device).map(d=>({...d,leaf_device:true}))}:gpuUUID?{...sample,gpus:sample.gpus.filter(g=>g.uuid===gpuUUID)}:sample)]);last=time;}return points;}

function sumGPU(h:FleetHostSnapshot,key:'pcie_rx_kb_per_second'|'pcie_tx_kb_per_second',scale:number) {const values=h.gpus.map(g=>g[key]);return values.length&&values.every((v):v is number=>v!=null)?values.reduce((sum,v)=>sum+v,0)/scale:null;}
