import type { DesiredDeployment, ManagementState, MeshNode, RecipeRevision } from '../../api/types';
export function servingModel(spec: Record<string, unknown>, fallback: string): string {
  if (Array.isArray(spec.distributed) && spec.distributed[0] && typeof spec.distributed[0] === 'object') return servingModel(spec.distributed[0] as Record<string,unknown>,fallback);
  const env = spec.environment as Record<string, unknown> | undefined;
  if (typeof env?.SERVED_MODEL_NAME === 'string') return env.SERVED_MODEL_NAME;
  const args = Array.isArray(spec.command) ? spec.command.map(String) : [];
  const index = args.indexOf('--served-model-name');
  if (index >= 0 && args[index + 1]) return args[index + 1]!;
  const match = args.join(' ').match(/--served-model-name\s+['"]?([^\s'"\\]+)/);
  return match?.[1] ?? fallback.replace(/-gpu[0-9]+$/, '');
}
export interface DeploymentRow { id: string; deployment: DesiredDeployment; host: string; revision?: RecipeRevision; model: string; }
export function deploymentRows(state: ManagementState | undefined, nodes: MeshNode[]): DeploymentRow[] {
  return Object.entries(state?.deployments ?? {}).map(([id, deployment]) => {
    const node = nodes.find(n => n.endpoint_id === deployment.endpoint_id);
    const revision = state?.recipes[deployment.recipe_id]?.find(r => r.digest === deployment.revision_digest);
    return {id, deployment, host: node?.host_snapshot?.hostname ?? node?.label ?? deployment.endpoint_id.slice(0,12), revision, model: servingModel(revision?.model_spec ?? {}, id)};
  });
}

export function runtimeSpec(row:DeploymentRow):Record<string,unknown>{
  const spec=row.revision?.model_spec??{};
  return placedSpec(spec,row.deployment);
}
export function placedSpec(spec:Record<string,unknown>,deployment:Pick<DesiredDeployment,'placement'>):Record<string,unknown>{
 const rank=Array.isArray(spec.distributed)?spec.distributed[deployment.placement?.rank??0] as Record<string,unknown>??{}:spec;
 return deployment.placement?.gpus?{...rank,gpus:`device=${deployment.placement.gpus.join(',')}`}:rank;
}
