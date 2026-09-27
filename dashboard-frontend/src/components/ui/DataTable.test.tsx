import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, within } from '@testing-library/react';
import { useState } from 'react';
import { DataTable } from './DataTable';
import type { DataTableColumn } from './dataTableModel';

type Row = { id: string; name: string; state: string };
const columns: DataTableColumn<Row>[] = [
  { id: 'name', label: 'Name', required: true, render: (row) => row.name },
  { id: 'state', label: 'State', render: (row) => row.state },
];
const rows: Row[] = [
  { id: 'a', name: 'Alpha', state: 'queued' },
  { id: 'b', name: 'Beta', state: 'running' },
  { id: 'c', name: 'Gamma', state: 'running' },
];

beforeEach(() => {
  const saved = new Map<string, string>();
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => { saved.set(key, value); },
  } });
});
afterEach(cleanup);

describe('DataTable', () => {
  it('filters, pages, persists visible columns, and expands a row', () => {
    const props = {
      id: 'base-test', rows, rowKey: (row: Row) => row.id, columns, rowTestId: 'base-row', clientPageSize: 1,
      filters: [{ id: 'state', label: 'State filter', options: [{ value: 'running', label: 'Running' }], matches: (row: Row, value: string) => row.state === value }],
      expandedKeys: ['b'], renderExpanded: (row: Row) => <div data-testid="expanded-detail">Detail {row.name}</div>,
    };
    const view = render(<DataTable {...props} />);
    expect(view.getAllByTestId('base-row')).toHaveLength(1);
    fireEvent.click(view.getByRole('button', { name: 'State filter' }));
    fireEvent.click(view.getByRole('checkbox', { name: 'Include State filter: Running' }));
    expect(view.getAllByTestId('base-row')[0]?.textContent).toContain('Beta');
    expect(view.getByTestId('expanded-detail').textContent).toBe('Detail Beta');
    fireEvent.click(within(view.getByTestId('base-test-pagination')).getByRole('button', { name: 'Next' }));
    expect(view.getAllByTestId('base-row')[0]?.textContent).toContain('Gamma');
    fireEvent.click(view.getByRole('button', { name: 'base-test columns' }));
    fireEvent.click(view.getByLabelText('State'));
    expect(view.queryByRole('columnheader', { name: 'State' })).toBeNull();
    expect(view.queryByRole('button', { name: 'State filter' })).toBeNull();
    expect(view.getByTestId('base-test-hidden-filters').textContent).toContain('State filter');
    fireEvent.click(within(view.getByTestId('base-test-hidden-filters')).getByRole('button', { name: 'Clear hidden' }));
    expect(view.getAllByTestId('base-row')[0]?.textContent).toContain('Alpha');
    view.unmount();
    const remounted = render(<DataTable {...props} />);
    expect(remounted.queryByRole('columnheader', { name: 'State' })).toBeNull();
    expect(remounted.getByRole('columnheader', { name: 'Name' })).toBeTruthy();
  });

  it('merges live rows by key and clamps a page when the population shrinks', () => {
    const view = render(<DataTable id="live-test" rows={rows.slice(0, 2)} rowKey={(row) => row.id} columns={columns}
      liveRows={[{ id: 'b', name: 'Beta live', state: 'decoding' }, { id: 'c', name: 'Gamma', state: 'queued' }]}
      clientPageSize={2} rowTestId="live-row" />);
    expect(view.getAllByTestId('live-row').map((row) => row.textContent)).toEqual(['Gammaqueued', 'Alphaqueued']);
    fireEvent.click(within(view.getByTestId('live-test-pagination')).getByRole('button', { name: 'Next' }));
    expect(view.getAllByTestId('live-row')[0]?.textContent).toBe('Beta livedecoding');
    view.rerender(<DataTable id="live-test" rows={rows.slice(0, 1)} rowKey={(row) => row.id} columns={columns}
      clientPageSize={2} rowTestId="live-row" />);
    expect(view.getAllByTestId('live-row')[0]?.textContent).toBe('Alphaqueued');
    expect(view.getByTestId('live-test-pagination').textContent).toContain('Page 1');
  });

  it('delegates expansion to the table while preserving row column alignment', () => {
    function Expandable() {
      const [expanded, setExpanded] = useState(false);
      return <DataTable id="expand-test" rows={rows.slice(0, 1)} rowKey={(row) => row.id} columns={columns}
        expandedKeys={expanded ? ['a'] : []} onToggleExpanded={() => setExpanded((value) => !value)}
        renderExpanded={(row) => <div data-testid="expanded-detail">{row.name} details</div>} />;
    }
    const view = render(<Expandable />);
    fireEvent.click(view.getByRole('button', { name: 'Expand a' }));
    expect(view.getByTestId('expanded-detail').textContent).toBe('Alpha details');
    expect(view.getByTestId('expand-test-expanded-row').querySelector('td')?.colSpan).toBe(2);
    fireEvent.click(view.getByRole('button', { name: 'Collapse a' }));
    expect(view.queryByTestId('expanded-detail')).toBeNull();
  });

  it('shares column visibility between mounted tables with the same column state key', () => {
    const view = render(<><DataTable id="mirror-a" columnStateKey="shared" rows={rows} rowKey={(row) => row.id} columns={columns} />
      <DataTable id="mirror-b" columnStateKey="shared" rows={rows} rowKey={(row) => row.id} columns={columns} /></>);
    fireEvent.click(view.getByRole('button', { name: 'mirror-a columns' }));
    fireEvent.click(within(view.getByTestId('mirror-a-column-chooser')).getByLabelText('State'));
    expect(within(view.getByTestId('mirror-b-data-table')).queryByRole('columnheader', { name: 'State' })).toBeNull();
  });
});
