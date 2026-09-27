import { Fragment, useMemo, useRef, useState, type ReactNode } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { cn } from '../../lib/cn';
import { FacetSelect } from './FacetSelect';
import { emptyFacet, hasFacet, type FacetSelection } from './facetModel';
import { mergeTableRows, type DataTableColumn, type DataTableFilter, type DataTablePagination } from './dataTableModel';
import { useTableColumns } from './useTableColumns';

const EMPTY_ROWS: readonly never[] = [];
const EMPTY_FILTERS: readonly never[] = [];
const EMPTY_KEYS: readonly string[] = [];

export interface DataTableProps<Row> {
  id: string;
  /** Tables with identical columns can share one persisted visibility choice. */
  columnStateKey?: string;
  rows: readonly Row[];
  rowKey: (row: Row) => string;
  columns: readonly DataTableColumn<Row>[];
  /** Extra filter controls, including server-side filters owned by the view. */
  filterBar?: ReactNode | ((visibleIds: readonly string[]) => ReactNode);
  filters?: readonly DataTableFilter<Row>[];
  pagination?: DataTablePagination;
  clientPageSize?: number;
  /** Live rows merge into the same keyed population without remounting the table. */
  liveRows?: readonly Row[];
  mergeLiveRow?: (stored: Row, live: Row) => Row;
  /** The virtual grid mode preserves a small DOM under large live populations. */
  virtualize?: { rowHeight: number; overscan?: number; scrollTestId?: string };
  renderRow?: (row: Row, columns: readonly DataTableColumn<Row>[]) => ReactNode;
  canExpand?: (row: Row) => boolean;
  expandedKeys?: readonly string[];
  onToggleExpanded?: (row: Row) => void;
  renderExpanded?: (row: Row, columns: readonly DataTableColumn<Row>[]) => ReactNode;
  /** Expanded children use the same table columns as their parent. */
  expandedRows?: (row: Row) => readonly Row[];
  expandOnRowClick?: boolean;
  showHeader?: boolean;
  onRowClick?: (row: Row) => void;
  selectedKey?: string | null;
  rowTestId?: string;
  rowClassName?: (row: Row) => string;
  rowAttributes?: (row: Row) => Record<string, string | undefined>;
  emptyMessage?: string;
  emptyContent?: ReactNode;
  className?: string;
  tableClassName?: string;
  tableTestId?: string;
  headerClassName?: string;
  controlsClassName?: string;
  minWidth?: number;
  showColumnChooser?: boolean;
  showControlsWhenEmpty?: boolean;
  sort?: { by: string; descending: boolean; onChange: (by: string, descending: boolean) => void };
}

export function DataTable<Row>({ id, columnStateKey = id, rows, rowKey, columns, filterBar, filters = EMPTY_FILTERS, pagination,
  clientPageSize, liveRows = EMPTY_ROWS, mergeLiveRow, virtualize, renderRow, canExpand, expandedKeys = EMPTY_KEYS,
  onToggleExpanded, renderExpanded, expandedRows, expandOnRowClick = false, showHeader = true, onRowClick, selectedKey,
  rowTestId, rowClassName, rowAttributes, emptyMessage = 'No rows.', emptyContent, className, tableClassName,
  headerClassName, controlsClassName, minWidth, tableTestId, showColumnChooser = true, showControlsWhenEmpty = true, sort,
}: DataTableProps<Row>) {
  const visibleIds = useTableColumns(columnStateKey, columns);
  const [showColumns, setShowColumns] = useState(false);
  const [filterValues, setFilterValues] = useState<Record<string, FacetSelection>>({});
  const [clientPage, setClientPage] = useState(1);
  const scrollRef = useRef<HTMLDivElement>(null);
  const visible = useMemo(() => columns.filter((column) => visibleIds.includes(column.id)), [columns, visibleIds]);
  const hiddenFilters = filters.filter((filter) => !visibleIds.includes(filter.id) && hasFacet(filterValues[filter.id] ?? emptyFacet()));
  const population = useMemo(() => liveRows.length
    ? mergeTableRows(rows, liveRows, rowKey, mergeLiveRow ?? ((_stored, live) => live)) : rows,
  [rows, liveRows, rowKey, mergeLiveRow]);
  const filtered = useMemo(() => population.filter((row) => filters.every((filter) => {
    const value = filterValues[filter.id] ?? emptyFacet();
    return (value.include.length === 0 || value.include.some((option) => filter.matches(row, option))) &&
      !value.exclude.some((option) => filter.matches(row, option));
  })), [population, filters, filterValues]);
  const effectivePage = clientPageSize ? Math.min(clientPage, Math.max(1, Math.ceil(filtered.length / clientPageSize))) : pagination?.page ?? 1;
  const displayed = clientPageSize
    ? filtered.slice((effectivePage - 1) * clientPageSize, effectivePage * clientPageSize)
    : filtered;
  const effectivePagination: DataTablePagination | undefined = clientPageSize
    ? { page: effectivePage, pageSize: clientPageSize, total: filtered.length,
      hasNext: effectivePage * clientPageSize < filtered.length,
      onPrevious: () => setClientPage(Math.max(1, effectivePage - 1)),
      onNext: () => setClientPage(effectivePage + 1) }
    : pagination;
  const virtualizer = useVirtualizer({
    count: displayed.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => virtualize?.rowHeight ?? 30,
    overscan: virtualize?.overscan ?? 12,
    enabled: !!virtualize,
  });
  const gridTemplate = visible.map((column) => column.width ?? 'minmax(100px,1fr)').join(' ');
  const tableMinWidth = minWidth ?? visible.reduce((width, column) => width + (Number(column.width?.match(/\d+/)?.[0]) || 160), 0);

  function toggleColumn(column: DataTableColumn<Row>) {
    if (column.required) return;
    const next = visibleIds.includes(column.id)
      ? visibleIds.filter((value) => value !== column.id)
      : [...visibleIds, column.id];
    if (next.length === 0) return;
    try { window.localStorage.setItem(`llmconduit.table.${columnStateKey}.columns`, JSON.stringify(next)); } catch { /* storage may be disabled */ }
    window.dispatchEvent(new CustomEvent('llmconduit:table-columns', { detail: { key: columnStateKey, ids: next } }));
  }

  function defaultRow(row: Row) {
    const key = rowKey(row);
    const attrs = rowAttributes?.(row) ?? {};
    const expandable = !!(renderExpanded || expandedRows) && (canExpand?.(row) ?? true);
    const expanded = expandedKeys.includes(key);
    const activate = () => { onRowClick?.(row); if (expandOnRowClick && expandable) onToggleExpanded?.(row); };
    return <Fragment key={key}><tr data-testid={rowTestId} data-selected={selectedKey === key || undefined} data-expanded={expanded || undefined}
      className={cn('border-b border-line/60', onRowClick && 'cursor-pointer hover:bg-accent/[0.06]', rowClassName?.(row))}
      onClick={onRowClick || expandOnRowClick ? activate : undefined}
      tabIndex={onRowClick || expandOnRowClick ? 0 : undefined}
      onKeyDown={onRowClick || expandOnRowClick ? (event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); activate(); } } : undefined}
      {...attrs}>
      {visible.map((column, index) => <td key={column.id} className={cn('px-2 py-2', column.align === 'right' && 'text-right tabular-nums', column.cellClassName)}>
        {index === 0 && expandable && <button type="button" aria-label={`${expanded ? 'Collapse' : 'Expand'} ${key}`} aria-expanded={expanded}
          onClick={(event) => { event.stopPropagation(); onToggleExpanded?.(row); }}
          className={cn('mr-1 inline-block text-text-muted transition-transform', expanded && 'rotate-90')}>›</button>}
        {column.render?.(row)}
      </td>)}
    </tr>{expanded && expandedRows?.(row).map((child) => <tr key={rowKey(child)} data-testid={`${id}-expanded-row`}
      className={cn('border-b border-line/60 bg-panel-raised/40', onRowClick && 'cursor-pointer hover:bg-accent/[0.06]')}
      onClick={() => onRowClick?.(child)}>
      {visible.map((column) => <td key={column.id} className={cn('px-2 py-2', column.align === 'right' && 'text-right tabular-nums', column.cellClassName)}>{column.render?.(child)}</td>)}
    </tr>)}{expanded && renderExpanded && !(expandedRows?.(row).length) && <tr data-testid={`${id}-expanded-row`} className="border-b border-line/60 bg-panel-raised/40"><td colSpan={visible.length} className="p-0">{renderExpanded(row, visible)}</td></tr>}</Fragment>;
  }

  const controls = (showControlsWhenEmpty || rows.length > 0) && (showColumnChooser || filters.length > 0 || filterBar) &&
    <div className={cn('flex flex-wrap items-center gap-2 border-b border-line bg-panel px-2 py-1.5 text-xs', controlsClassName)} data-testid={`${id}-table-controls`}>
      {typeof filterBar === 'function' ? filterBar(visibleIds) : filterBar}
      {filters.filter((filter) => visibleIds.includes(filter.id)).map((filter) => <FacetSelect key={filter.id} label={filter.label} options={filter.options}
        value={filterValues[filter.id] ?? emptyFacet()}
        onChange={(value) => { setFilterValues({ ...filterValues, [filter.id]: value }); setClientPage(1); }} />)}
      {hiddenFilters.length > 0 && <span className="text-xs text-status-cooling" data-testid={`${id}-hidden-filters`}>
        Hidden filters: {hiddenFilters.map((filter) => filter.label).join(', ')}
        <button type="button" className="ml-2 underline hover:text-text" onClick={() => {
          setFilterValues(Object.fromEntries(Object.entries(filterValues).filter(([key]) => !hiddenFilters.some((filter) => filter.id === key))));
          setClientPage(1);
        }}>Clear hidden</button>
      </span>}
      {showColumnChooser && <div className="relative ml-auto">
        <button type="button" aria-expanded={showColumns} aria-label={`${id} columns`} onClick={() => setShowColumns((open) => !open)}
          className="rounded border border-line px-2 py-1 text-text-muted hover:text-text">Columns · {visible.length}/{columns.length}</button>
        {showColumns && <div className="absolute right-0 top-full z-30 mt-1 max-h-72 min-w-44 overflow-auto rounded border border-line bg-panel p-2 shadow-xl" data-testid={`${id}-column-chooser`}>
          {columns.map((column) => <label key={column.id} className="flex items-center gap-2 whitespace-nowrap px-1 py-0.5 text-text">
            <input type="checkbox" checked={visibleIds.includes(column.id)} disabled={column.required}
              onChange={() => toggleColumn(column)} />{column.label || column.id}
          </label>)}
        </div>}
      </div>}
    </div>;

  return <div className={cn('flex min-h-0 min-w-0 flex-1 flex-col', className)} data-testid={`${id}-data-table`}>
    {controls}
    {virtualize ? <>
      {showHeader && <div className={cn('grid border-b border-line bg-panel-raised px-3 py-1.5 text-[10px] uppercase tracking-[0.14em] text-text-muted', headerClassName)}
        style={{ gridTemplateColumns: gridTemplate, gap: '0.5rem' }} role="row">
        {visible.map((column) => <span key={column.id} role="columnheader" title={column.title}
          className={cn(column.align === 'right' && 'text-right', column.headerClassName)}>{column.label}</span>)}
      </div>}
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto" data-testid={virtualize.scrollTestId}>
        <div style={{ height: `${virtualizer.getTotalSize()}px`, position: 'relative', width: '100%' }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = displayed[item.index];
            if (!row) return null;
            return <div key={rowKey(row)} data-index={item.index} data-testid={rowTestId}
              style={{ position: 'absolute', top: 0, left: 0, width: '100%', height: `${virtualize.rowHeight}px`, transform: `translateY(${item.start}px)` }}>
              {renderRow?.(row, visible)}
            </div>;
          })}
        </div>
        {displayed.length === 0 && (emptyContent ?? <div className="px-3 py-6 text-center text-xs text-text-muted">{emptyMessage}</div>)}
      </div>
    </> : <div className="min-h-0 flex-1 overflow-auto">
      {displayed.length === 0 && (emptyContent ?? <div className="p-4 text-xs text-text-muted">{emptyMessage}</div>)}
      {displayed.length > 0 && <table className={cn('w-full table-fixed text-left text-xs', tableClassName)} style={{ minWidth: tableMinWidth }} data-testid={tableTestId}>
        {showHeader && <thead className={cn('sticky top-0 z-10 bg-panel-raised text-[10px] uppercase tracking-wider text-text-muted', headerClassName)}>
          <tr className="border-b border-line">{visible.map((column) => <th key={column.id}
            className={cn('px-2 py-2', column.align === 'right' && 'text-right', column.headerClassName)}
            style={{ width: column.width }} title={column.title}>{column.sortable && sort
              ? <button type="button" className="inline-flex items-center gap-1 hover:text-text" aria-label={`Sort ${column.label}`}
                  onClick={() => sort.onChange(column.id, sort.by === column.id ? !sort.descending : true)}>
                  {column.label}<span aria-hidden="true">{sort.by === column.id ? sort.descending ? '↓' : '↑' : '↕'}</span>
                </button> : column.label}</th>)}</tr>
        </thead>}
        <tbody>{displayed.map((row) => renderRow ? renderRow(row, visible) : defaultRow(row))}</tbody>
      </table>}
    </div>}
    {effectivePagination && <div className="flex items-center gap-3 border-t border-line bg-panel px-3 py-2 text-xs text-text-muted" data-testid={`${id}-pagination`}>
      <span>Page {effectivePagination.page}</span>
      <span>{displayed.length} shown · {effectivePagination.pageSize} per page{effectivePagination.total != null ? ` · ${effectivePagination.total} total` : ''}</span>
      <div className="ml-auto flex gap-2">
        <button type="button" disabled={effectivePagination.page === 1 || effectivePagination.busy} onClick={effectivePagination.onPrevious}
          className="rounded border border-line px-2 py-1 disabled:opacity-40">{effectivePagination.previousLabel ?? 'Previous'}</button>
        <button type="button" disabled={!effectivePagination.hasNext || effectivePagination.busy} onClick={effectivePagination.onNext}
          className="rounded border border-line px-2 py-1 disabled:opacity-40">{effectivePagination.nextLabel ?? 'Next'}</button>
      </div>
    </div>}
  </div>;
}
