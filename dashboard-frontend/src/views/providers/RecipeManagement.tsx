import { useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { getConnection } from '../../api/connection';
import type { MeshNode, SelectDeployment } from '../../api/types';
import { Panel } from '../../components/ui/Panel';
import { Button } from '../../components/ui/Button';
const key = ['management'] as const;
const input = 'rounded border border-line bg-bg px-2 py-1.5 text-xs text-text';
export function RecipeManagement({ nodes, mutationsEnabled }: { nodes: MeshNode[]; mutationsEnabled: boolean }) {
  const { client } = getConnection();
  const cache = useQueryClient();
  const state = useQuery({ queryKey: key, queryFn: () => client.management(), retry: false, refetchInterval: 5_000 });
  const [recipeId, setRecipeId] = useState('');
  const [description, setDescription] = useState('');
  const [spec, setSpec] = useState('{\n  "image": ""\n}');
  const [deploymentId, setDeploymentId] = useState('');
  const [host, setHost] = useState('');
  const [revisionKey, setRevisionKey] = useState('');
  const [formError, setFormError] = useState('');
  const refresh = () => { void cache.invalidateQueries({ queryKey: key }); };
  const create = useMutation({ mutationFn: (body: { description: string; model_spec: Record<string, unknown> }) => client.createRecipeRevision(recipeId, body), onSuccess: refresh });
  const select = useMutation({ mutationFn: ({ id, body }: { id: string; body: SelectDeployment }) => client.selectDeployment(id, body), onSuccess: refresh });
  const busy = create.isPending || select.isPending;
  const revisions = Object.entries(state.data?.recipes ?? {}).flatMap(([id, versions]) => versions.map((revision) => ({ id, revision })));
  function save(event: FormEvent) {
    event.preventDefault();
    try {
      const model: unknown = JSON.parse(spec);
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
  return <Panel title="Recipes and deployments" data-testid="recipe-management">
    <div className="space-y-4 text-xs">
      <p className="text-text-muted">Save immutable revisions, then select a revision and host to deploy. Hosts enforce their local policy. Recipe edits leave running deployments unchanged.</p>
      {state.error && <p role="alert">{String(state.error)}</p>}
      {!mutationsEnabled && <p className="text-text-muted">Deployment changes are disabled for this session.</p>}
      <form onSubmit={save} className="space-y-2">
        <div className="flex flex-wrap gap-2">
          <input aria-label="Recipe ID" placeholder="Recipe ID" required pattern="[a-z0-9][a-z0-9_-]{0,39}" className={input} value={recipeId} onChange={(e) => setRecipeId(e.target.value)} />
          <input aria-label="Recipe description" placeholder="Description" required maxLength={256} className={input} value={description} onChange={(e) => setDescription(e.target.value)} />
        </div>
        <textarea aria-label="Recipe specification" rows={7} className={`${input} w-full font-mono`} value={spec} onChange={(e) => setSpec(e.target.value)} />
        <Button type="submit" disabled={!mutationsEnabled || busy}>Save revision</Button>
      </form>
      <div className="space-y-1">{revisions.map(({ id, revision }) => <div key={`${id}:${revision.digest}`}><strong>{id}</strong> · revision {revision.revision} · {String(revision.model_spec.image ?? revision.model_spec.source_format ?? '')}<span className="ml-2 text-text-muted">{revision.digest.slice(0, 12)}</span><Button onClick={() => { setRecipeId(id); setDescription(revision.description); setSpec(JSON.stringify(revision.model_spec, null, 2)); }}>Edit revision</Button></div>)}</div>
      <form onSubmit={deploy} className="flex flex-wrap gap-2">
        <input aria-label="Deployment ID" placeholder="Deployment ID" required pattern="[a-z0-9][a-z0-9_-]{0,39}" className={input} value={deploymentId} onChange={(e) => setDeploymentId(e.target.value)} />
        <select aria-label="Deployment host" required className={input} value={host} onChange={(e) => setHost(e.target.value)}><option value="">Select host</option>{nodes.filter((node) => node.enabled && node.managed_deployments).map((node) => <option key={node.endpoint_id} value={node.endpoint_id}>{node.label ?? node.host_snapshot?.hostname ?? node.endpoint_id.slice(0, 12)}</option>)}</select>
        <select aria-label="Recipe revision" required className={input} value={revisionKey} onChange={(e) => setRevisionKey(e.target.value)}><option value="">Select revision</option>{revisions.filter(({ revision }) => revision.model_spec.image).map(({ id, revision }) => <option key={`${id}:${revision.digest}`} value={`${id}:${revision.digest}`}>{id} · revision {revision.revision}</option>)}</select>
        <Button type="submit" disabled={!mutationsEnabled || busy}>Deploy</Button>
      </form>
      {Object.entries(state.data?.deployments ?? {}).map(([id, deployment]) => {
        const observed = deployment.observed?.status?.phase ?? 'Awaiting host';
        const stale = deployment.observed_at_ms == null || Date.now() - deployment.observed_at_ms > 20_000;
        return <div key={id} className="rounded border border-line p-3 space-y-2">
          <div><strong>{id}</strong> · {nodes.find((node) => node.endpoint_id === deployment.endpoint_id)?.label ?? deployment.endpoint_id.slice(0, 12)} · {deployment.recipe_id} · {deployment.revision_digest.slice(0, 12)}</div>
          <div>Desired: {deployment.desired_state} · Observed: {observed}{stale ? ' (stale)' : ''} · generation {deployment.generation}</div>
          {(deployment.error || deployment.observed?.error || deployment.observed?.status?.last_error) && <p role="alert">{deployment.error ?? deployment.observed?.error ?? deployment.observed?.status?.last_error}</p>}
          <Button disabled={!mutationsEnabled || busy} onClick={() => select.mutate({ id, body: { endpoint_id: deployment.endpoint_id, recipe_id: deployment.recipe_id, revision_digest: deployment.revision_digest, expected_generation: deployment.generation, desired_state: deployment.desired_state === 'ready' ? 'unloaded' : 'ready' } })}>{deployment.desired_state === 'ready' ? 'Stop' : 'Start'}</Button>
        </div>;
      })}
      {(formError || create.error || select.error) && <p role="alert">{formError || String(create.error ?? select.error)}</p>}
    </div>
  </Panel>;
}
