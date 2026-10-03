import { useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { getConnection } from '../../api/connection';
import type { MeshNode, SelectDeployment } from '../../api/types';
import { Panel } from '../../components/ui/Panel';
import { Button } from '../../components/ui/Button';
import { RecipeImport } from './RecipeImport';
import { DataTable } from '../../components/ui/DataTable';
const key = ['management'] as const;
const input = 'rounded border border-line bg-bg px-2 py-1.5 text-xs text-text';
export function RecipeManagement({ nodes, mutationsEnabled, mode = 'all' }: { nodes: MeshNode[]; mutationsEnabled: boolean; mode?: 'all' | 'recipes' | 'deployment' }) {
  const { client } = getConnection();
  const cache = useQueryClient();
  const state = useQuery({ queryKey: key, queryFn: () => client.management(), retry: false, refetchInterval: 5_000 });
  const [recipeId, setRecipeId] = useState('');
  const [description, setDescription] = useState('');
  const [format, setFormat] = useState<'fleet' | 'compose' | 'sparkrun'>('fleet');
  const [spec, setSpec] = useState('{\n  "image": ""\n}');
  const [deploymentId, setDeploymentId] = useState('');
  const [host, setHost] = useState('');
  const [revisionKey, setRevisionKey] = useState('');
  const [recipeQuery, setRecipeQuery] = useState('');
  const [formError, setFormError] = useState('');
  const refresh = () => { void cache.invalidateQueries({ queryKey: key }); };
  const create = useMutation({ mutationFn: (body: { description: string; model_spec: Record<string, unknown> }) => client.createRecipeRevision(recipeId, body), onSuccess: refresh });
  const select = useMutation({ mutationFn: ({ id, body }: { id: string; body: SelectDeployment }) => client.selectDeployment(id, body), onSuccess: refresh });
  const busy = create.isPending || select.isPending;
  const revisions = Object.entries(state.data?.recipes ?? {}).flatMap(([id, versions]) => versions.map((revision) => ({ id, revision })));
  async function upload(file: File | undefined) {
    if (!file) return;
    if (file.size > 32 * 1024) { setFormError('Recipe files must be at most 32 KiB.'); return; }
    try { setSpec(await file.text()); setFormError(''); }
    catch { setFormError('The recipe file could not be read.'); }
  }
  function save(event: FormEvent) {
    event.preventDefault();
    try {
      const model: unknown = format === 'fleet' ? JSON.parse(spec) : { source_format: format, source_content: spec };
      if (!model || typeof model !== 'object' || Array.isArray(model)) throw new Error('Recipe specification must be an object.');
      setFormError('');
      create.mutate({ description, model_spec: model as Record<string, unknown> });
    } catch (error) { setFormError(String(error)); }
  }
  function deploy(event: FormEvent) {
    event.preventDefault();
    const selected = revisions.find(({ id, revision }) => `${id}:${revision.digest}` === revisionKey);
    if (!selected) return;
    const current = state.data?.deployments[deploymentId];
    select.mutate({ id: deploymentId, body: { expected_generation: current?.generation ?? 0, endpoint_id: host, recipe_id: selected.id, revision_digest: selected.revision.digest, desired_state: 'ready' } });
  }
  return <Panel title={mode === 'recipes' ? 'Recipe catalog' : mode === 'deployment' ? 'Deployment controls' : 'Recipes and deployments'} data-testid="recipe-management">
    <div className="space-y-4 text-xs">
      <p className="text-text-muted">Save immutable revisions, then select a revision and host to deploy. Hosts enforce their local policy. Recipe edits leave running deployments unchanged.</p>
      {state.error && <p role="alert">{String(state.error)}</p>}
      {!mutationsEnabled && <p className="text-text-muted">Deployment changes are disabled for this session.</p>}
      {mode !== 'deployment' && <>
      <form onSubmit={save} className="space-y-2">
        <div className="flex flex-wrap gap-2">
          <input aria-label="Recipe ID" placeholder="Recipe ID" required pattern="[a-z0-9][a-z0-9_-]{0,39}" className={input} value={recipeId} onChange={(e) => setRecipeId(e.target.value)} />
          <input aria-label="Recipe description" placeholder="Description" required maxLength={256} className={input} value={description} onChange={(e) => setDescription(e.target.value)} />
        </div>
        <label className="block">Recipe format <select aria-label="Recipe format" className={input} value={format} onChange={(e) => setFormat(e.target.value as typeof format)}><option value="fleet">Fleet JSON</option><option value="compose">Compose YAML</option><option value="sparkrun">Sparkrun YAML</option></select></label>
        {format !== 'fleet' && <label className="block">Upload recipe YAML <input aria-label="Upload recipe YAML" type="file" accept=".yaml,.yml" onChange={(e) => { void upload(e.target.files?.[0]); }} /></label>}
        <textarea aria-label="Recipe specification" rows={7} className={`${input} w-full font-mono`} value={spec} onChange={(e) => setSpec(e.target.value)} />
        <Button type="submit" disabled={!mutationsEnabled || busy}>{format === 'fleet' ? 'Save revision' : 'Save source'}</Button>
      </form>
      {format !== 'fleet' && <RecipeImport recipeId={recipeId} description={description} sourceFormat={format} sourceContent={spec} mutationsEnabled={mutationsEnabled} onSaved={refresh} />}
      <DataTable id="recipe-revisions" tableTestId="recipe-revisions" rows={revisions.filter(r => `${r.id} ${r.revision.description}`.toLowerCase().includes(recipeQuery.toLowerCase()))} filterBar={<input aria-label="Search recipes" placeholder="Search recipe or description…" className={input} value={recipeQuery} onChange={e=>setRecipeQuery(e.target.value)} />} rowKey={({id, revision}) => `${id}:${revision.digest}`} clientPageSize={20} minWidth={800} columns={[
        {id:'id', label:'Recipe', required:true, render: ({id}) => <strong>{id}</strong>},
        {id:'revision', label:'Revision', render: ({revision}) => revision.revision},
        {id:'format', label:'Format', render: ({revision}) => String(revision.model_spec.source_format ?? 'Fleet')},
        {id:'description', label:'Description', render: ({revision}) => revision.description},
        {id:'created', label:'Saved', render: ({revision}) => new Date(revision.created_at_ms).toLocaleString()},
        {id:'edit', label:'Review / edit', render: ({id, revision}) => <Button onClick={() => { setRecipeId(id); setDescription(revision.description);
          const source = revision.model_spec.source_format;
          if ((source === 'compose' || source === 'sparkrun') && typeof revision.model_spec.source_content === 'string') { setFormat(source); setSpec(revision.model_spec.source_content); }
          else { setFormat('fleet'); setSpec(JSON.stringify(revision.model_spec, null, 2)); }
        }}>Edit revision</Button>},
      ]} emptyMessage="No recipes saved." />
      </>}
      {mode !== 'recipes' && <>
      <form onSubmit={deploy} className="flex flex-wrap gap-2">
        <input aria-label="Deployment ID" placeholder="Deployment ID" required pattern="[a-z0-9][a-z0-9_-]{0,39}" className={input} value={deploymentId} onChange={(e) => setDeploymentId(e.target.value)} />
        <select aria-label="Deployment host" required className={input} value={host} onChange={(e) => setHost(e.target.value)}><option value="">Select host</option>{nodes.filter((node) => node.enabled && node.managed_deployments).map((node) => <option key={node.endpoint_id} value={node.endpoint_id}>{node.label ?? node.host_snapshot?.hostname ?? node.endpoint_id.slice(0, 12)}</option>)}</select>
        <select aria-label="Recipe revision" required className={input} value={revisionKey} onChange={(e) => setRevisionKey(e.target.value)}><option value="">Select revision</option>{revisions.filter(({ revision }) => revision.model_spec.image).map(({ id, revision }) => <option key={`${id}:${revision.digest}`} value={`${id}:${revision.digest}`}>{id} · revision {revision.revision}</option>)}</select>
        <Button type="submit" disabled={!mutationsEnabled || busy}>Deploy</Button>
      </form>
      <DataTable id="deployment-controls" tableTestId="deployment-controls" rows={Object.entries(state.data?.deployments ?? {}).map(([id, deployment]) => ({id, deployment}))} rowKey={({id}) => id} minWidth={900} columns={[
        {id:'id', label:'Deployment', required:true, render: ({id}) => <strong>{id}</strong>},
        {id:'host', label:'Host', render: ({deployment}) => nodes.find(n => n.endpoint_id === deployment.endpoint_id)?.host_snapshot?.hostname ?? deployment.endpoint_id.slice(0,12)},
        {id:'recipe', label:'Recipe / revision', render: ({deployment}) => `${deployment.recipe_id} · ${deployment.revision_digest.slice(0,12)}`},
        {id:'desired', label:'Desired', render: ({deployment}) => deployment.desired_state},
        {id:'observed', label:'Observed', render: ({deployment}) => <span>{deployment.observed?.status?.phase ?? 'Awaiting host'}{deployment.observed_at_ms == null || Date.now() - deployment.observed_at_ms > 20_000 ? ' (stale)' : ''}</span>},
        {id:'error', label:'Error', render: ({deployment}) => deployment.error ?? deployment.observed?.error ?? deployment.observed?.status?.last_error ?? '—'},
        {id:'action', label:'Action', render: ({id, deployment}) => <Button disabled={!mutationsEnabled || busy} onClick={() => select.mutate({ id, body: { endpoint_id: deployment.endpoint_id, recipe_id: deployment.recipe_id, revision_digest: deployment.revision_digest, expected_generation: deployment.generation, desired_state: deployment.desired_state === 'ready' ? 'unloaded' : 'ready' } })}>{deployment.desired_state === 'ready' ? 'Stop' : 'Start'}</Button>},
      ]} emptyMessage="No deployments selected." />
      </>}
      {(formError || create.error || select.error) && <p role="alert">{formError || String(create.error ?? select.error)}</p>}
    </div>
  </Panel>;
}
