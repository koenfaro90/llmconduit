export type RequestColumn = 'number' | 'start' | 'end' | 'id' | 'user' | 'key' | 'harness' | 'session' | 'status' | 'ttft' | 'duration' | 'tokensIn' | 'cached' | 'newIn' | 'tokensOut' | 'decodeSpeed' | 'model' | 'provider' | 'protocol' | 'kind' | 'lineage';

export const ALL_REQUEST_COLUMNS: readonly RequestColumn[] = [
  'number', 'id', 'start', 'end', 'user', 'key', 'harness', 'session', 'status', 'ttft', 'duration',
  'tokensIn', 'newIn', 'tokensOut', 'decodeSpeed', 'model', 'provider', 'protocol', 'kind',
];

export const DEFAULT_HIDDEN_REQUEST_COLUMNS: readonly RequestColumn[] = ['id', 'protocol', 'kind'];
