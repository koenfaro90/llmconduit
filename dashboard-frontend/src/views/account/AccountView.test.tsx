import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { AccountView } from './AccountView';
import { renderWithQuery, resetWorld } from '../../components/testHarness';
import { authStore } from '../../store/authStore';
import { getConnection } from '../../api/connection';

describe('AccountView (mock accounts API)', () => {
  beforeEach(() => {
    resetWorld({ mock: true });
    // The connection seeds the auth store from the bootstrap; build it first, then set the user.
    getConnection();
    authStore.getState().setCsrfToken('mock-csrf-token');
  });
  afterEach(cleanup);

  it('an admin sees everyone’s keys and the users panel; creating a key shows the secret once', async () => {
    authStore.getState().setAuthMode('users');
    authStore.getState().setUser({ id: 'user_admin', username: 'admin', is_admin: true });
    const { getByTestId, getAllByTestId, getByLabelText, getByRole, queryByTestId } = renderWithQuery(<AccountView />);
    expect(getByTestId('account-username').textContent).toBe('admin');
    expect(getByTestId('account-role').textContent).toBe('admin');
    await waitFor(() => expect(getAllByTestId('key-row').length).toBe(2));
    await waitFor(() => expect(getAllByTestId('user-row').length).toBe(2));

    fireEvent.change(getByLabelText('key label'), { target: { value: 'desk' } });
    fireEvent.change(getByLabelText('allowed models'), { target: { value: 'local, fast' } });
    fireEvent.click(getByRole('button', { name: 'create key' }));
    await waitFor(() => expect(queryByTestId('account-new-secret')).toBeTruthy());
    expect(getByTestId('account-secret').textContent).toMatch(/^llmc_/);
    await waitFor(() => expect(getAllByTestId('key-row').length).toBe(3));
    const created = getAllByTestId('key-row').find((row) => row.textContent?.includes('desk'))!;
    expect(created.textContent).toContain('local, fast');

    // Revoke it again.
    fireEvent.click(within(created).getByTestId('key-revoke'));
    await waitFor(() => expect(getAllByTestId('key-row').length).toBe(2));

    // Create a user; the admin cannot delete themselves.
    fireEvent.change(getByLabelText('new username'), { target: { value: 'ops' } });
    fireEvent.change(getByLabelText('new password'), { target: { value: 'ops-password' } });
    fireEvent.click(getByRole('button', { name: 'create user' }));
    await waitFor(() => expect(getAllByTestId('user-row').length).toBe(3));
    const self = getAllByTestId('user-row').find((row) => row.textContent?.includes('(you)'))!;
    expect(within(self).getByTestId('user-delete')).toBeDisabled();
  });

  it('a plain user sees only their own keys and no users panel', async () => {
    authStore.getState().setAuthMode('users');
    authStore.getState().setUser({ id: 'user_dev', username: 'dev', is_admin: false });
    const { getByTestId, getAllByTestId, queryByTestId } = renderWithQuery(<AccountView />);
    expect(getByTestId('account-role').textContent).toBe('user');
    await waitFor(() => expect(getAllByTestId('key-row').length).toBe(1));
    expect(getAllByTestId('key-row')[0]!.textContent).toContain('ci');
    expect(queryByTestId('account-users')).toBeNull();
  });
  it('edits metadata and ownership in place, then rotates with a copy-once token', async () => {
    authStore.getState().setUser({id:'user_admin',username:'admin',is_admin:true});
    const view=renderWithQuery(<AccountView />);
    await waitFor(()=>expect(view.getAllByTestId('key-row')).toHaveLength(2));
    const row=view.getAllByTestId('key-row').find(r=>r.textContent?.includes('laptop'))!;
    const id=row.getAttribute('title');
    fireEvent.click(within(row).getByTestId('key-edit'));
    const editor=view.getByTestId('key-editor');
    fireEvent.change(within(editor).getByLabelText('Key label'),{target:{value:'Workstation'}});
    fireEvent.change(within(editor).getByLabelText('Key owner'),{target:{value:'user_dev'}});
    fireEvent.change(within(editor).getByLabelText('New Allowed models'),{target:{value:'custom-alias'}});
    fireEvent.click(within(editor).getByRole('button',{name:/^Add$/}));
    fireEvent.click(within(editor).getByRole('button',{name:'Save key changes'}));
    await waitFor(()=>expect(view.queryByTestId('key-editor')).toBeNull());
    const updated=view.getAllByTestId('key-row').find(r=>r.textContent?.includes('Workstation'))!;
    expect(updated.getAttribute('title')).toBe(id);
    expect(updated.textContent).toContain('dev');
    expect(updated.textContent).toContain('custom-alias');
    expect(view.queryByTestId('account-new-secret')).toBeNull();
    fireEvent.click(within(updated).getByTestId('key-rotate'));
    fireEvent.click(view.getByRole('button',{name:'Generate replacement token'}));
    await waitFor(()=>expect(view.getByTestId('account-secret').textContent).toMatch(/^llmc_mock_rotated/));
    fireEvent.click(view.getByRole('button',{name:'dismiss'}));
    expect(view.queryByTestId('account-secret')).toBeNull();
    expect(JSON.stringify(getConnection().queryClient.getMutationCache().getAll().map(m=>m.state.data))).not.toContain('llmc_mock_rotated');
    expect(view.getAllByTestId('key-row')).toHaveLength(2);
  });

  it('keeps a draft on stale-write errors and discards it explicitly', async()=>{
    authStore.getState().setUser({id:'user_admin',username:'admin',is_admin:true});
    const view=renderWithQuery(<AccountView />);
    await waitFor(()=>expect(view.getAllByTestId('key-row')).toHaveLength(2));
    const keys=await getConnection().client.listKeys('all');const key=keys.keys[0]!;
    fireEvent.click(within(view.getAllByTestId('key-row')[0]!).getByTestId('key-edit'));
    await getConnection().client.updateKey(key.id,{label:'Other admin',user_id:key.user_id,allowed_models:key.allowed_models,expected_updated_at_ms:key.updated_at_ms});
    fireEvent.change(within(view.getByTestId('key-editor')).getByLabelText('Key label'),{target:{value:'My draft'}});
    fireEvent.click(view.getByRole('button',{name:'Save key changes'}));
    await waitFor(()=>expect(view.getByRole('alert').textContent).toContain('changed'));
    expect(within(view.getByTestId('key-editor')).getByLabelText('Key label')).toHaveValue('My draft');
    fireEvent.click(view.getByRole('button',{name:'Discard key changes'}));
    expect(view.queryByTestId('key-editor')).toBeNull();
    await waitFor(()=>expect(view.getByText('Other admin')).toBeTruthy());
  });

  it('renames an account while retaining its keys and identity',async()=>{
    authStore.getState().setUser({id:'user_admin',username:'admin',is_admin:true});const view=renderWithQuery(<AccountView/>);
    await waitFor(()=>expect(view.getByRole('button',{name:'Rename dev'})).toBeTruthy());fireEvent.click(view.getByRole('button',{name:'Rename dev'}));const editor=view.getByTestId('username-editor');fireEvent.change(within(editor).getByLabelText('Sign-in username'),{target:{value:'developer'}});fireEvent.click(within(editor).getByRole('button',{name:'Save username'}));await waitFor(()=>expect(view.queryByTestId('username-editor')).toBeNull());const users=await getConnection().client.listUsers();expect(users.users.find(u=>u.id==='user_dev')?.username).toBe('developer');expect((await getConnection().client.listKeys('all')).keys.find(k=>k.id==='key_dev_ci')?.user_id).toBe('user_dev');
  });

});
