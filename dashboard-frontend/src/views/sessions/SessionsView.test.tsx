import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { SessionsView } from './SessionsView';
import { renderWithQuery, resetWorld } from '../../components/testHarness';
import { flowFilterStore } from '../../store/flowFilterStore';

describe('SessionsView (mock APIs)', () => {
  beforeEach(() => { resetWorld({ mock: true }); window.location.hash = '#/sessions'; });
  afterEach(cleanup);

  it('shows active five-minute sessions above every retained session with the requested metrics', async () => {
    const ui = renderWithQuery(<SessionsView />);
    const active = ui.getByTestId('active-sessions-section');
    const all = ui.getByTestId('all-sessions-section');
    await waitFor(() => expect(within(active).getAllByTestId('session-row')).toHaveLength(3));
    await waitFor(() => expect(within(all).getAllByTestId('session-row')).toHaveLength(3));
    expect(within(all).getByText('first activity')).toBeTruthy();
    expect(within(all).getByText('last activity')).toBeTruthy();
    expect(within(all).getByText('tokens IN')).toBeTruthy();
    expect(within(all).getByText('tokens OUT')).toBeTruthy();
    expect(within(all).getByText('in flight')).toBeTruthy();
    expect(ui.queryByTestId('sessions-tree-toggle')).toBeNull();
    expect(ui.queryByText('Select a session to see its sub-sessions and requests.')).toBeNull();
    expect(ui.getByTestId('sessions-tree').firstElementChild?.classList.contains('w-full')).toBe(true);
  });

  it('keeps expanded sub-session cells in the parent table columns and opens the normal request viewer', async () => {
    const ui = renderWithQuery(<SessionsView />);
    const all = ui.getByTestId('all-sessions-section');
    await waitFor(() => expect(within(all).getAllByTestId('session-row')).toHaveLength(3));
    const root = within(all).getAllByTestId('session-row').find((row) => row.textContent?.includes('S-1'))!;
    fireEvent.click(root);
    await ui.findByTestId('session-detail');
    expect(ui.getByTestId('sessions-tree').firstElementChild?.classList.contains('w-[42%]')).toBe(true);
    await waitFor(() => expect(within(all).getAllByTestId('session-roots-expanded-row')).toHaveLength(1));
    const child = within(all).getByTestId('session-roots-expanded-row');
    expect(child.querySelectorAll('td')).toHaveLength(root.querySelectorAll('td').length);
    const requests = within(ui.getByTestId('session-requests')).getAllByTestId('session-request');
    fireEvent.click(requests[0]!);
    await ui.findByTestId('flow-detail');
    expect(ui.getByTestId('flow-detail').textContent).toContain('R-');
  });

  it('sorts and filters the complete table, and expands an active session into the shared request table', async () => {
    const ui = renderWithQuery(<SessionsView />);
    const all = ui.getByTestId('all-sessions-section');
    const active = ui.getByTestId('active-sessions-section');
    await waitFor(() => expect(within(all).getAllByTestId('session-row')).toHaveLength(3));
    fireEvent.click(within(all).getByRole('button', { name: 'Sort tokens IN' }));
    await waitFor(() => expect(within(all).getAllByTestId('session-row')[0]?.textContent).toContain('S-3'));
    fireEvent.click(ui.getByRole('button', { name: 'Harness' }));
    fireEvent.click(ui.getByRole('checkbox', { name: 'Include Harness: codex' }));
    await waitFor(() => expect(within(all).getAllByTestId('session-row')).toHaveLength(1));
    const activeRow = within(active).getAllByTestId('session-row')[0]!;
    fireEvent.click(activeRow);
    const expansion = await ui.findByTestId('active-session-requests');
    expect(within(expansion).getAllByTestId('active-request')).toHaveLength(2);
    fireEvent.click(within(expansion).getByTestId('active-session-show-in-flows'));
    expect(flowFilterStore.getState().filters.session).toBe('sess_codex');
  });
});
