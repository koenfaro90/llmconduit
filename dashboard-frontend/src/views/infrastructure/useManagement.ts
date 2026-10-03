import { useQuery } from '@tanstack/react-query';
import type { MeshNode } from '../../api/types';
const EMPTY_NODES: MeshNode[] = [];
import { getConnection, queryKeys } from '../../api/connection';
export function useManagement() {
  const {client} = getConnection();
  const mesh = useQuery({queryKey: queryKeys.mesh, queryFn: () => client.mesh(), retry:false, refetchInterval:5_000});
  const management = useQuery({queryKey:['management'], queryFn: () => client.management(), retry:false, refetchInterval:5_000});
  return {nodes:mesh.data?.nodes ?? EMPTY_NODES, state:management.data, error:mesh.error ?? management.error, loading:mesh.isLoading || management.isLoading};
}
