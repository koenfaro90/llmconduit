import { useState, type ChangeEvent } from 'react';
import { useMutation } from '@tanstack/react-query';
import { getConnection } from '../../api/connection';
import type { ConvertedRecipe, ConvertRecipeRequest, RecipeImportOptions } from '../../api/types';
import { Button } from '../../components/ui/Button';
const input = 'rounded border border-line bg-bg px-2 py-1.5 text-xs text-text';

export function RecipeImport({ recipeId, description, sourceFormat, sourceContent, mutationsEnabled, onSaved }: {
  recipeId: string; description: string; sourceFormat: 'compose' | 'sparkrun'; sourceContent: string;
  mutationsEnabled: boolean; onSaved: () => void;
}) {
  const { client } = getConnection();
  const [options, setOptions] = useState<Record<string, string>>({});
  const [variables, setVariables] = useState('{}');
  const [environment, setEnvironment] = useState('{}');
  const [preview, setPreview] = useState<{ request: ConvertRecipeRequest; response: ConvertedRecipe } | null>(null);
  const [message, setMessage] = useState('');
  function request(): ConvertRecipeRequest {
    const parsed: RecipeImportOptions = {};
    for (const [key, value] of Object.entries(options)) {
      if (!value.trim()) continue;
      if (key === 'environment_files' || key === 'devices') parsed[key] = value.split('\n').map((s) => s.trim()).filter(Boolean);
      else if (key === 'num_nodes' || key === 'node_rank') {
        const number = Number(value);
        if (!Number.isInteger(number) || number < 0 || (key === 'num_nodes' && number === 0)) throw new Error('Invalid node count or rank');
        parsed[key] = number;
      }
      else Object.assign(parsed, { [key]: value.trim() });
    }
    const vars: unknown = JSON.parse(variables);
    const env: unknown = JSON.parse(environment);
    if (!vars || typeof vars !== 'object' || Array.isArray(vars) || !env || typeof env !== 'object' || Array.isArray(env) || Object.values(env).some((v) => typeof v !== 'string')) {
      throw new Error('Variables must be a JSON object; environment overrides must map names to strings.');
    }
    parsed.variables = vars as Record<string, unknown>;
    parsed.environment = env as Record<string, string>;
    return { source_format: sourceFormat, source_content: sourceContent, options: parsed };
  }
  let current: ConvertRecipeRequest | null = null;
  try { current = request(); } catch { /* Parse errors are shown on Preview, without sending a request. */ }
  const currentKey = JSON.stringify(current);
  const matches = preview != null && current != null && JSON.stringify(preview.request) === currentKey;
  const convert = useMutation({ mutationFn: (body: ConvertRecipeRequest) => client.convertRecipe(body), onSuccess: (response, body) => { setPreview({ request: body, response }); setMessage(''); } });
  const save = useMutation({ mutationFn: async () => {
    if (!preview || !matches || preview.response.required_inputs.length) throw new Error('Refresh the preview and resolve missing inputs before saving.');
    // Keep the editable source and executable result under the same immutable recipe history.
    await client.createRecipeRevision(recipeId, { description, model_spec: { source_format: preview.request.source_format, source_content: preview.request.source_content } });
    return client.createRecipeRevision(recipeId, { description, model_spec: preview.response.model_spec });
  }, onSuccess: () => { onSaved(); setMessage('Source and portable revision saved. Select the recipe and ordered hosts on the Deployment page.'); } });
  const busy = convert.isPending || save.isPending;
  function change(key: string) { return (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => { setOptions((old) => ({ ...old, [key]: event.target.value })); setMessage(''); }; }
  function previewSource() {
    try { const body = request(); setMessage(''); convert.mutate(body); }
    catch { setMessage('Conversion variables must be valid JSON objects. Environment values must be strings.'); }
  }
  const fields: Array<[string, string, string]> = [
    ['service', 'Compose service', 'Leave blank for a single-service file'],
    ['image', 'Pinned image', 'repository@sha256:… or sha256:image-id'],
    ['user', 'Container user', 'e.g. 1000:1000 or 0:0'],
    ['memory_limit', 'RAM limit', 'e.g. 116000m or 64g'],
    ['gpus', 'GPU devices', 'e.g. device=0 or device=0,1'],
    ['base_directory', 'Recipe directory', 'Absolute directory on the target host'],
    ['home_directory', 'Host home directory', 'e.g. /home/koen'],
    ['readiness_url', 'Readiness URL', 'Optional host-loopback HTTP probe'],
  ];
  if (sourceFormat === 'sparkrun') fields.push(['runtime_cache_directory', 'Runtime cache directory', 'Optional existing approved cache path'], ['num_nodes', 'Distributed node count', 'Defaults to the recipe minimum'], ['node_rank', 'Distributed node rank', 'Leave blank to convert all ranks into one portable recipe'], ['head_address', 'Distributed head address', 'Head fabric IP'], ['host_address', 'Distributed host address', 'This rank’s fabric IP']);
  return <div className="space-y-2" data-testid="recipe-import">
    <p className="text-text-muted">Convert YAML to a Fleet recipe. Preview preserves commands, environment, bind paths and ports. Missing settings and unsupported features block saving a deployable revision.</p>
    <div className="grid gap-2 md:grid-cols-2">{fields.filter(([key]) => key !== 'service' || sourceFormat === 'compose').map(([key, label, placeholder]) => <label key={key} className="space-y-1"><span>{label}</span><input aria-label={label} className={`${input} block w-full`} placeholder={placeholder} value={options[key] ?? ''} onChange={change(key)} /></label>)}</div>
    <details><summary className="cursor-pointer">Variables, environment and secret files</summary><div className="space-y-2 pt-2">
      <label className="block">Variables / sparkrun overrides (JSON)<textarea aria-label="Conversion variables" className={`${input} block w-full font-mono`} rows={3} value={variables} onChange={(e) => setVariables(e.target.value)} /></label>
      <label className="block">Environment overrides (JSON)<textarea aria-label="Conversion environment" className={`${input} block w-full font-mono`} rows={3} value={environment} onChange={(e) => setEnvironment(e.target.value)} /></label>
      <label className="block">Host-local environment files, one absolute path per line<textarea aria-label="Host environment files" className={`${input} block w-full font-mono`} rows={2} value={options.environment_files ?? ''} onChange={change('environment_files')} /></label>
      <label className="block">Additional host devices, one per line<textarea aria-label="Conversion devices" className={`${input} block w-full font-mono`} rows={2} value={options.devices ?? ''} onChange={change('devices')} /></label>
      <p className="text-text-muted">Secrets stay in approved private files on the host. Do not paste credentials into the recipe or variables.</p>
    </div></details>
    <Button onClick={previewSource} disabled={!mutationsEnabled || busy || !sourceContent.trim()}>Preview conversion</Button>
    {preview && <div className="rounded border border-line p-3 space-y-2">
      {!matches && <p role="status">The source or settings changed. Refresh the preview before saving.</p>}
      {preview.response.services.length > 1 && <p>Services in this file: {preview.response.services.join(', ')}. Convert and deploy each required service separately.</p>}
      {preview.response.required_inputs.length > 0 && <div><strong>Resolve before saving</strong><ul className="list-disc pl-5">{preview.response.required_inputs.map((item) => <li key={item}>{item}</li>)}</ul></div>}
      {preview.response.warnings.length > 0 && <ul className="list-disc pl-5 text-text-muted">{preview.response.warnings.map((item) => <li key={item}>{item}</li>)}</ul>}
      <textarea aria-label="Converted Fleet specification" readOnly rows={10} className={`${input} w-full font-mono`} value={JSON.stringify(preview.response.model_spec, null, 2)} />
      <Button onClick={() => save.mutate()} disabled={!mutationsEnabled || busy || !matches || preview.response.required_inputs.length > 0 || !/^[a-z0-9][a-z0-9_-]{0,39}$/.test(recipeId) || !description.trim()}>Save converted revision</Button>
    </div>}
    {message && <p role="status">{message}</p>}
    {(convert.error || save.error) && <p role="alert">{String(convert.error ?? save.error)}</p>}
  </div>;
}
