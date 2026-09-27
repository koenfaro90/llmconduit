import type { DivergenceKind } from '../../api/types';

/** Copy shared by request rows and the standard request inspector. */
export function divergenceLabel(kind: DivergenceKind | null | undefined): { label: string; bust: boolean; title: string } {
  switch (kind) {
    case 'append':
      return { label: 'append', bust: false, title: 'extends the predecessor: only new items were added' };
    case 'instructions_changed':
      return { label: 'instructions', bust: true, title: 'the system/instructions block changed inside the shared prefix' };
    case 'tools_changed':
      return { label: 'tools', bust: true, title: 'the tool list changed inside the shared prefix' };
    case 'history_rewritten':
      return { label: 'rewritten', bust: true, title: 'earlier conversation history changed or was removed' };
    case 'new_chain':
      return { label: 'new', bust: false, title: 'the start of a conversation: nothing in common with a known chain' };
    default:
      return { label: '—', bust: false, title: 'lineage not computed' };
  }
}
