import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { HostTelemetry } from './HostTelemetry';
import type { MeshNode } from '../../api/types';
afterEach(cleanup);
const now = Date.parse('2026-10-03T12:00:00Z');
const node: MeshNode = { endpoint_id:'node-1', enabled:true, joined_at_ms:now, host_snapshot:{
  schema_version:1, sampled_at:'2026-10-03T12:00:00Z', hostname:'gbspark01', architecture:'arm64',
  uptime_seconds:100, cpu:null, memory:null, disk:null, network:[], containers:[], errors:[],
  gpus:[{index:0,uuid:'GPU-1',name:'NVIDIA GB10',unified_memory:true,memory_total_mib:null,
    memory_used_mib:null,utilization_percent:null,temperature_celsius:40,power_watts:null}],
} };
describe('HostTelemetry', () => {
  it('preserves unavailable values and identifies unified memory', () => {
    render(<HostTelemetry nodes={[node]} now={now} />);
    const panel = screen.getByTestId('host-telemetry');
    expect(panel).toHaveTextContent('unified memory (host RAM)');
    expect(panel).not.toHaveTextContent('0.0 GiB');
    expect(panel.querySelector('[data-quality="measured"]')).not.toBeNull();
  });
  it('shows small nonzero container memory without rounding it to zero', () => {
    const measured: MeshNode = { ...node, host_snapshot: { ...node.host_snapshot!, network:[{name:'eth0',rx_bytes:1,tx_bytes:1,rx_bytes_per_second:0.5,tx_bytes_per_second:100}], containers: [{
      id:'container', name:'inference', image:'vllm:test', state:'running', status:'Up',
      cpu_percent:null, memory_bytes:20 * 1024 ** 2, pids:8,
    }] } };
    render(<HostTelemetry nodes={[measured]} now={now} />);
    expect(screen.getByTestId('host-telemetry')).toHaveTextContent('20.0 MiB');
    expect(screen.getByTestId('host-telemetry')).toHaveTextContent('0.5 B/s');
  });
  it('marks old measurements as stale and disconnected hosts unavailable', () => {
    render(<HostTelemetry nodes={[node, { ...node, endpoint_id:'node-2', host_snapshot:undefined }]} now={now + 21_000} />);
    expect(screen.getByText(/stale sample/)).toBeInTheDocument();
    expect(screen.getByText('Host telemetry unavailable.')).toBeInTheDocument();
  });
});

it('shows PCIe measurements and independent collection timestamps',()=>{
 const measured:MeshNode={...node,host_snapshot:{...node.host_snapshot!,collection:{host_interval_ms:100,gpu_interval_ms:100,container_interval_ms:5000,gpu_backend:'nvml',gpu_sampled_at:'2026-10-03T12:00:00Z',containers_sampled_at:null,gpu_duration_ms:45},gpus:[{...node.host_snapshot!.gpus[0]!,pcie_rx_kb_per_second:1000000,pcie_tx_kb_per_second:0,pcie_link_generation:5,pcie_link_width:16,pcie_replay_count:0}]}};
 render(<HostTelemetry nodes={[measured]} now={now}/>);
 expect(screen.getByTestId('host-telemetry')).toHaveTextContent('PCIe RX 1000.00 MB/s · TX 0.00 MB/s');
 expect(screen.getByTestId('host-telemetry')).toHaveTextContent('link Gen5 ×16');
 expect(screen.getByTestId('host-telemetry')).toHaveTextContent('GPU tick 100 ms (nvml)');
 expect(screen.getByTestId('host-telemetry')).toHaveTextContent('correctable errors —');
});

it('shows independent metric intervals and disabled container resource sampling',()=>{
 const measured:MeshNode={...node,host_snapshot:{...node.host_snapshot!,collection:{host_interval_ms:100,gpu_interval_ms:100,container_interval_ms:5000,gpu_backend:'nvml',gpu_sampled_at:null,containers_sampled_at:null,gpu_duration_ms:null,metrics:{pcie_rx:{interval_ms:100,sampled_at:'2026-10-03T12:00:00Z'},container_cpu:{interval_ms:0,sampled_at:null},memory:{interval_ms:30000,sampled_at:'2026-10-03T11:59:40Z'}}}}};
 render(<HostTelemetry nodes={[measured]} now={now}/>);
 expect(screen.getByText('Metric sampling intervals')).toBeInTheDocument();
 const table=screen.getByRole('table',{hidden:true});
 expect(table).toHaveTextContent('container cpu');expect(table).toHaveTextContent('Off');expect(table).toHaveTextContent('30000 ms');
});

it('shows fan percentages per physical GPU without clamping or inventing unavailable readings',()=>{
 const measured:MeshNode={...node,host_snapshot:{...node.host_snapshot!,gpus:[{...node.host_snapshot!.gpus[0]!,fan_speed_percent:123},{...node.host_snapshot!.gpus[0]!,index:1,uuid:'second',fan_speed_percent:0},{...node.host_snapshot!.gpus[0]!,index:2,uuid:'third',fan_speed_percent:null}]}};
 render(<HostTelemetry nodes={[measured]} now={now}/>);
 const panel=screen.getByTestId('host-telemetry');expect(panel).toHaveTextContent('Fan 123.0%');expect(panel).toHaveTextContent('Fan 0.0%');expect(panel).toHaveTextContent('Fan —');
});
