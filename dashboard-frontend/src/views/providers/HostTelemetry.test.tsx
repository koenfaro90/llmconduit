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
  it('marks old measurements as stale and disconnected hosts unavailable', () => {
    render(<HostTelemetry nodes={[node, { ...node, endpoint_id:'node-2', host_snapshot:undefined }]} now={now + 21_000} />);
    expect(screen.getByText(/stale sample/)).toBeInTheDocument();
    expect(screen.getByText('Host telemetry unavailable.')).toBeInTheDocument();
  });
});
