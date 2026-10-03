import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { renderWithQuery, resetWorld } from '../../components/testHarness';
import { ProvidersView } from './ProvidersView';
import { RecipeManagement } from './RecipeManagement';
import { getConnection } from '../../api/connection';

beforeEach(() => resetWorld({ mock: true }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('ProvidersView', () => {
  it('edits archived source revisions without offering them for deployment', async () => {
    const source = { source_format: 'compose', source_content: 'services: {}\n' };
    vi.spyOn(getConnection().client, 'management').mockResolvedValue({ recipes: {
      archived: [{ digest: 'a'.repeat(64), revision: 1, description: 'Archived Compose', model_spec: source, created_at_ms: 1 }],
      executable: [{ digest: 'b'.repeat(64), revision: 1, description: 'Executable', model_spec: { image: 'image@sha256:' + 'b'.repeat(64) }, created_at_ms: 1 }],
    }, deployments: {} });
    renderWithQuery(<RecipeManagement nodes={[]} mutationsEnabled />);
    const row = (await screen.findByText('archived')).closest('tr')!;
    fireEvent.click(within(row).getByRole('button', { name: 'Edit revision' }));
    expect(screen.getByLabelText('Recipe ID')).toHaveValue('archived');
    expect(screen.getByLabelText('Recipe format')).toHaveValue('compose');
    expect(screen.getByLabelText('Recipe specification')).toHaveValue(source.source_content);
    expect(within(screen.getByLabelText('Recipe revision')).queryByRole('option', { name: /archived/ })).toBeNull();
    expect(within(screen.getByLabelText('Recipe revision')).getByRole('option', { name: /executable/ })).toBeInTheDocument();
  });
  it('renders exact advertised models, operational capacity, access limits, and cache metrics', async () => {
    renderWithQuery(<ProvidersView />);

    await screen.findByTestId('providers-view');
    await waitFor(() => expect(screen.getAllByTestId('provider-row').length).toBeGreaterThan(0));

    const table = screen.getByTestId('providers-table');
    expect(table).toHaveTextContent('vllm-a');
    expect(table).toHaveTextContent('llama-3.1-70b');
    expect(table).toHaveTextContent('2/8 active');
    expect(table).toHaveTextContent('America/Chicago');
    expect(table).toHaveTextContent('4 concurrent sessions');
    expect(table).toHaveTextContent('hit%');
    expect(table).toHaveTextContent('kv');
    expect(screen.getByTestId('providers-catalog')).toHaveTextContent('Exact provider-scoped model advertisements');
  });

  it('renders Fleet state and sends CSRF-gated load/unload actions', async () => {
    renderWithQuery(<ProvidersView />);

    const panel = await screen.findByTestId('fleet-panel');
    await waitFor(() => expect(within(panel).getAllByTestId('fleet-model-card').length).toBeGreaterThan(0));
    expect(panel).toHaveTextContent('qwen3-8b-flash');
    expect(panel).toHaveTextContent('GPU 0');

    const idleCard = within(panel).getByText('qwen3-32b').closest('[data-testid="fleet-model-card"]') as HTMLElement;
    fireEvent.click(within(idleCard).getByRole('button', { name: 'Load' }));

    await waitFor(() => expect(idleCard).toHaveTextContent('loading'));
  });

  it('discovers and removes additional OpenAI-compatible providers', async () => {
    renderWithQuery(<ProvidersView />);

    const panel = await screen.findByTestId('configured-providers-panel');
    expect(panel).toHaveTextContent('Local lab');
    expect(panel).not.toHaveTextContent('secret-value');

    fireEvent.change(within(panel).getByLabelText('Provider name'), { target: { value: 'Remote lab' } });
    fireEvent.change(within(panel).getByLabelText('Provider URL'), { target: { value: 'https://inference.example/v1' } });
    fireEvent.change(within(panel).getByLabelText('Provider API key'), { target: { value: 'secret-value' } });
    fireEvent.click(within(panel).getByRole('button', { name: 'Discover & add' }));

    await waitFor(() => expect(panel).toHaveTextContent('Remote lab'));
    expect(panel).not.toHaveTextContent('secret-value');
  });

  it('renders and invokes model switching advertised by a downstream mesh worker', async () => {
    renderWithQuery(<ProvidersView />);

    const panel = await screen.findByTestId('mesh-admin');
    await waitFor(() => expect(panel).toHaveTextContent('remote model switching'));
    const model = within(panel).getByText('qwen3-32b').closest('[data-testid="remote-switch-model"]') as HTMLElement;
    fireEvent.click(within(model).getByRole('button', { name: 'Switch' }));

    await waitFor(() => expect(model).toHaveTextContent('loading'));
  });
});
