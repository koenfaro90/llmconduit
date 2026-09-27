import { useEffect, useState } from 'react';
import { initialVisibleColumns, type DataTableColumn } from './dataTableModel';

/** Column visibility shared by a table and controls mounted outside it. */
export function useTableColumns<Row>(key: string, columns: readonly DataTableColumn<Row>[]): string[] {
  const [ids, setIds] = useState(() => initialVisibleColumns(columns, key));
  useEffect(() => {
    const sync = (event: Event) => {
      const detail = (event as CustomEvent<{ key: string; ids: string[] }>).detail;
      if (detail?.key === key) setIds(detail.ids);
    };
    window.addEventListener('llmconduit:table-columns', sync);
    return () => window.removeEventListener('llmconduit:table-columns', sync);
  }, [key]);
  return ids;
}
