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
