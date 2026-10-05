export interface AliasMapping { id: string; name: string; profiles: string[] }
export interface AliasView extends AliasMapping { error: string | null }
export interface AliasTarget { id: string; name: string; upstream_models: string[]; backends: string[]; available: boolean }
export interface AliasesResponse { revision: number; aliases: AliasView[]; targets: AliasTarget[]; persistent: boolean }
const object = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);
const strings = (v: unknown): v is string[] => Array.isArray(v) && v.every(x => typeof x === 'string');
export function isAliasesResponse(v: unknown): v is AliasesResponse {
  return object(v) && Number.isSafeInteger(v.revision) && Number(v.revision) >= 0 && typeof v.persistent === 'boolean'
    && Array.isArray(v.aliases) && v.aliases.every(a => object(a) && typeof a.id === 'string' && typeof a.name === 'string' && strings(a.profiles) && (a.error === null || typeof a.error === 'string'))
    && Array.isArray(v.targets) && v.targets.every(t => object(t) && typeof t.id === 'string' && typeof t.name === 'string' && strings(t.upstream_models) && strings(t.backends) && typeof t.available === 'boolean');
}
