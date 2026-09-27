import type { ReactNode } from 'react';

export interface DataTableColumn<Row> {
  id: string;
  label: string;
  width?: string;
  align?: 'left' | 'right';
  title?: string;
  /** Used by ordinary tables. Specialized rows may render their own cells. */
  render?: (row: Row) => ReactNode;
  cellClassName?: string;
  headerClassName?: string;
  required?: boolean;
  defaultHidden?: boolean;
  sortable?: boolean;
}

export interface DataTableFilter<Row> {
  id: string;
  label: string;
  options: Array<{ value: string; label: string }>;
  matches: (row: Row, value: string) => boolean;
}

export interface DataTablePagination {
  page: number;
  pageSize: number;
  hasNext: boolean;
  onPrevious: () => void;
  onNext: () => void;
  previousLabel?: string;
  nextLabel?: string;
  total?: number;
  busy?: boolean;
}

export function initialVisibleColumns<Row>(columns: readonly DataTableColumn<Row>[], storageKey: string): string[] {
  const defaults = columns.filter((column) => !column.defaultHidden || column.required).map((column) => column.id);
  const all = columns.map((column) => column.id);
  try {
    const saved = JSON.parse(window.localStorage.getItem(`llmconduit.table.${storageKey}.columns`) ?? 'null');
    if (!Array.isArray(saved)) return defaults;
    const known = new Set(all);
    const visible = saved.filter((id): id is string => typeof id === 'string' && known.has(id));
    for (const column of columns) if (column.required && !visible.includes(column.id)) visible.push(column.id);
    return visible.length ? all.filter((id) => visible.includes(id)) : defaults;
  } catch {
    return defaults;
  }
}

export function mergeTableRows<Row>(rows: readonly Row[], liveRows: readonly Row[], rowKey: (row: Row) => string,
  merge: (stored: Row, live: Row) => Row): Row[] {
  const liveById = new Map(liveRows.map((row) => [rowKey(row), row]));
  const storedIds = new Set(rows.map(rowKey));
  return [
    ...liveRows.filter((row) => !storedIds.has(rowKey(row))),
    ...rows.map((row) => {
      const live = liveById.get(rowKey(row));
      return live ? merge(row, live) : row;
    }),
  ];
}
