import { useAuth } from '../../store/hooks';
import { RecipeManagement } from '../providers/RecipeManagement';
import { useManagement } from './useManagement';
export function RecipesView() {
  const {nodes} = useManagement(); const mutationsEnabled = useAuth(s => s.mutationsEnabled);
  return <div className="min-w-0 flex-1 overflow-auto p-4 space-y-4" data-testid="recipes-view"><header><h1 className="text-lg font-semibold">Recipes</h1><p className="text-xs text-text-muted">Review immutable revisions, edit recipes, or import Compose and sparkrun YAML.</p></header><RecipeManagement nodes={nodes} mutationsEnabled={mutationsEnabled} mode="recipes" /></div>;
}
