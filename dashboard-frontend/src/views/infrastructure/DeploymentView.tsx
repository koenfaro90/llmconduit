import { useAuth } from '../../store/hooks';
import { RecipeManagement } from '../providers/RecipeManagement';
import { useManagement } from './useManagement';
export function DeploymentView() {
  const {nodes,error} = useManagement(); const mutationsEnabled = useAuth(s => s.mutationsEnabled);
  return <div className="min-w-0 flex-1 overflow-auto p-4 space-y-4" data-testid="deployment-view"><header><h1 className="text-lg font-semibold">Deployment</h1><p className="text-xs text-text-muted">Choose a recipe revision and host. Fleet applies the host's execution policy.</p></header>{error && <p role="alert">{String(error)}</p>}<RecipeManagement nodes={nodes} mutationsEnabled={mutationsEnabled} mode="deployment" /></div>;
}
