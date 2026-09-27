import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, within } from '@testing-library/react';
import { FilterBar } from './FilterBar';
import { EMPTY_FILTERS, type FlowFilters } from './filterTypes';

afterEach(cleanup);

function renderBar(over: Partial<Parameters<typeof FilterBar>[0]> = {}) {
  const onChange = vi.fn();
  const props = { filters: EMPTY_FILTERS as FlowFilters, models: ['gpt-4o'], upstreams: ['vllm-a'],
    clients: [] as string[], total: 3, shown: 3, onChange, ...over };
  return { ...render(<FilterBar {...props} />), onChange };
}

describe('FilterBar shared facets', () => {
  it('keeps a cross-linked model visible even after it leaves the option list', () => {
    const { container, onChange } = renderBar({ filters: { ...EMPTY_FILTERS, model: 'claude-x' } });
    fireEvent.click(within(container).getByRole('button', { name: 'Model' }));
    expect(within(container).getByRole('checkbox', { name: 'Include Model: claude-x' }).getAttribute('checked')).not.toBeNull();
    fireEvent.click(within(container).getByRole('checkbox', { name: 'Exclude Model: gpt-4o' }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ model: null,
      facets: expect.objectContaining({ model: { include: ['claude-x'], exclude: ['gpt-4o'] } }) }));
  });

  it('offers the full client population through search without rendering thousands of rows', () => {
    const clients = Array.from({ length: 500 }, (_, i) => `client-${String(i).padStart(3, '0')}`);
    const { container } = renderBar({ clients });
    fireEvent.click(within(container).getByRole('button', { name: 'Client' }));
    expect(within(container).getByText('Showing 200 of 500; search to narrow.')).toBeTruthy();
    expect(within(container).queryByRole('checkbox', { name: 'Include Client: client-499' })).toBeNull();
    fireEvent.change(within(container).getByRole('textbox', { name: 'Search client' }), { target: { value: 'client-499' } });
    expect(within(container).getByRole('checkbox', { name: 'Include Client: client-499' })).toBeTruthy();
  });

  it('applies include and exclude immediately and clears all facets', () => {
    const { container, onChange } = renderBar();
    fireEvent.click(within(container).getByRole('button', { name: 'Status' }));
    fireEvent.click(within(container).getByRole('checkbox', { name: 'Include Status: failed' }));
    const next = onChange.mock.calls[0]![0] as FlowFilters;
    expect(next.facets.status).toEqual({ include: ['failed'], exclude: [] });
    const rendered = renderBar({ filters: next });
    fireEvent.click(within(rendered.container).getByTestId('flow-filter-clear'));
    expect(rendered.onChange).toHaveBeenCalledWith(EMPTY_FILTERS);
  });
});
