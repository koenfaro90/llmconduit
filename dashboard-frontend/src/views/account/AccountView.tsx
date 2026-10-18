/**
 * AccountView — who you are, your API keys (create / edit / rotate / revoke; the secret is shown once), and,
 * for administrators, user management (create, reset password, role, delete) plus everyone's
 * keys. Mutations carry the double-submit CSRF token through the client.
 */
import { useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { getConnection, queryKeys } from '../../api/connection';
import { useAuth } from '../../store/hooks';
import type { ApiKeyRecord, UserRecord, UpdateKeyRequest } from '../../api/types';
import { SettingsField } from '../settings/SettingsField';
import { Panel, PanelHeader } from '../../components/ui/Panel';
import { Button } from '../../components/ui/Button';
import { DataTable } from '../../components/ui/DataTable';
import type { DataTableColumn } from '../../components/ui/dataTableModel';
import { cn } from '../../lib/cn';

const DASH = '—';

function fmtWhen(ms: number): string {
  return new Date(ms).toISOString().slice(0, 16).replace('T', ' ');
}

export function AccountView() {
  const { client } = getConnection();
  const queryClient = useQueryClient();
  const user = useAuth((s) => s.user);
  const authMode = useAuth((s) => s.authMode);
  const me = useQuery({ queryKey: queryKeys.me, queryFn: () => client.me(), retry: false });
  const isAdmin = user?.is_admin ?? me.data?.is_admin ?? false;
  const accountsEnabled = me.data?.accounts_enabled ?? true;
  const keys = useQuery({ queryKey: queryKeys.keys(isAdmin ? 'all' : 'mine'), queryFn: () => client.listKeys(isAdmin ? 'all' : user?.id), retry: false, enabled: accountsEnabled });
  const users = useQuery({ queryKey: queryKeys.users, queryFn: () => client.listUsers(), enabled: isAdmin && accountsEnabled, retry: false });
  const [editingKey, setEditingKey] = useState<ApiKeyRecord | null>(null);
  const [rotatingKey, setRotatingKey] = useState<ApiKeyRecord | null>(null);
  const [copied, setCopied] = useState(false);
  const [secret, setSecret] = useState<{ label: string | null; secret: string } | null>(null);
  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ['keys'] });
    void queryClient.invalidateQueries({ queryKey: queryKeys.users });
  };
  const createKey = useMutation({
    mutationFn: async (body: { label?: string; allowed_models?: string[]; user_id?: string }) => {
      const created = await client.createKey(body);
      setCopied(false);
      setSecret({label: created.key.label, secret: created.secret});
      // Keep credentials out of TanStack's retained mutation response cache.
    },
    onSuccess: invalidate,
  });
  const revokeKey = useMutation({ mutationFn: (id: string) => client.deleteKey(id), onSuccess: invalidate });
  const updateKey = useMutation({mutationFn: (body: UpdateKeyRequest) => client.updateKey(editingKey!.id,body), onSuccess:()=>{setEditingKey(null);invalidate();},onError:invalidate});
  const rotateKey = useMutation({
    mutationFn: async (key: ApiKeyRecord) => {
      const created = await client.rotateKey(key.id,key.updated_at_ms);
      setCopied(false);
      setSecret({label:created.key.label,secret:created.secret});
    },
    onSuccess:()=>{setRotatingKey(null);invalidate();},onError:invalidate,
  });
  const createUser = useMutation({ mutationFn: (body: { username: string; password: string; is_admin: boolean }) => client.createUser(body), onSuccess: invalidate });
  const updateUser = useMutation({ mutationFn: (args: { id: string; password?: string; is_admin?: boolean }) => client.updateUser(args.id, { password: args.password, is_admin: args.is_admin }), onSuccess: invalidate });
  const deleteUser = useMutation({ mutationFn: (id: string) => client.deleteUser(id), onSuccess: invalidate });
  const userName = (id: string | null) => (id == null ? DASH : users.data?.users.find((u) => u.id === id)?.username ?? (user && user.id === id ? user.username : id.slice(0, 8)));

  return (
    <div className="min-h-0 min-w-0 flex-1 overflow-auto p-4" data-testid="account-view">
      <div className="mb-3 flex items-baseline gap-2">
        <h1 className="text-sm font-semibold uppercase tracking-[0.18em] text-text">account</h1>
        <span className="text-[10px] uppercase tracking-[0.14em] text-text-muted">you · keys · users</span>
      </div>

      <Panel raised className="mb-3 flex flex-wrap items-center gap-4 px-3 py-2" data-testid="account-me">
        <span className="font-mono text-sm text-text" data-testid="account-username">{user?.username ?? (authMode === 'open' ? 'dev-open session' : 'token session')}</span>
        <span className={cn('rounded-sm px-1 text-[9px] uppercase tracking-wide', isAdmin ? 'bg-accent/15 text-accent' : 'bg-line/40 text-text-muted')} data-testid="account-role">
          {isAdmin ? 'admin' : 'user'}
        </span>
        <span className="text-xs text-text-muted" data-testid="account-auth-mode">auth: {me.data?.auth_mode ?? authMode}</span>
        {!accountsEnabled && (
          <span className="text-xs text-status-cooling" data-testid="account-disabled">
            user and key management need a SQL store (control_plane.storage sqlite or postgres)
          </span>
        )}
      </Panel>

      {secret && (
        <Panel className="mb-3 border-status-healthy/40 px-3 py-2" data-testid="account-new-secret">
          <div className="text-[10px] uppercase tracking-[0.14em] text-status-healthy">new key{secret.label ? ` · ${secret.label}` : ''} — copy it now, it is shown once</div>
          <code className="mt-1 block select-all break-all font-mono text-sm text-text" data-testid="account-secret">{secret.secret}</code>
          <div className="mt-2 flex justify-end gap-2"><Button variant="ghost" onClick={()=>{void navigator.clipboard?.writeText(secret.secret).then(()=>setCopied(true)).catch(()=>setCopied(false));}}>{copied?'Copied':'Copy token'}</Button><Button variant="ghost" onClick={() => {setSecret(null);setCopied(false);createKey.reset();rotateKey.reset();}}>dismiss</Button></div>
        </Panel>
      )}

      <section className="mb-4" data-testid="account-keys">
        <PanelHeader label="API keys" help="Keys authenticate model clients. Empty allowed models means any model. Administrators can transfer ownership. Editing preserves the token and history; rotation replaces the token immediately and shows it once. Running requests continue."><h2 className="text-xs font-semibold">{isAdmin ? 'API keys · all users' : 'My API keys'}</h2></PanelHeader>
        <KeyForm
          busy={createKey.isPending}
          error={createKey.error ? String(createKey.error) : null}
          admin={isAdmin}
          users={users.data?.users ?? []}
          onCreate={(body) => createKey.mutate(body)}
        />
        {editingKey && <KeyEditor key={editingKey.id} record={editingKey} users={users.data?.users??[]} admin={isAdmin} busy={updateKey.isPending} error={updateKey.error?String(updateKey.error):null} onSave={body=>updateKey.mutate(body)} onCancel={()=>{setEditingKey(null);updateKey.reset();}} />}
        {rotatingKey && <Panel raised className="mb-3 space-y-3 p-3" role="dialog" aria-modal="false" aria-label="Rotate API key"><PanelHeader label="Token rotation" help="Generate a new random token for the same key ID. The old token stops authenticating new requests immediately. Replace it in clients; active requests continue." actions={<><Button variant="ghost" disabled={rotateKey.isPending} onClick={()=>{setRotatingKey(null);rotateKey.reset();}}>Cancel rotation</Button><Button disabled={rotateKey.isPending} onClick={()=>rotateKey.mutate(rotatingKey)}>Generate replacement token</Button></>}><h3 className="font-semibold">Rotate {rotatingKey.label??'unlabelled key'}</h3></PanelHeader><p className="text-xs text-text-muted">The old token will stop working immediately. The replacement is shown once.</p>{rotateKey.error&&<p role="alert" className="text-xs text-status-down">{String(rotateKey.error)}</p>}</Panel>}
        {(revokeKey.error||keys.error)&&<p role="alert" className="text-xs text-status-down">{String(revokeKey.error??keys.error)}</p>}
        <KeyTable keys={keys.data?.keys ?? []} ownerName={userName} onEdit={key=>{setEditingKey(key);updateKey.reset();setRotatingKey(null);}} onRotate={key=>{setSecret(null);setRotatingKey(key);rotateKey.reset();setEditingKey(null);}} onRevoke={id=>revokeKey.mutate(id)} busy={revokeKey.isPending||updateKey.isPending||rotateKey.isPending} />
      </section>

      {isAdmin && (
        <section data-testid="account-users">
          <h2 className="mb-1 text-[10px] uppercase tracking-[0.14em] text-text-muted">users</h2>
          <UserForm busy={createUser.isPending} error={createUser.error ? String(createUser.error) : null} onCreate={(body) => createUser.mutate(body)} />
          <UserTable
            users={users.data?.users ?? []}
            selfId={user?.id ?? null}
            keys={keys.data?.keys ?? []}
            onResetPassword={(id, password) => updateUser.mutate({ id, password })}
            onToggleAdmin={(id, is_admin) => updateUser.mutate({ id, is_admin })}
            onDelete={(id) => deleteUser.mutate(id)}
          />
          {(updateUser.error || deleteUser.error) && <p className="mt-1 text-xs text-status-down" data-testid="account-users-error">{String(updateUser.error ?? deleteUser.error)}</p>}
        </section>
      )}
    </div>
  );
}

const INPUT = 'rounded-md border border-line bg-panel-raised px-2 py-1 font-mono text-xs text-text outline-none focus:border-accent';

function KeyForm({ busy, error, admin, users, onCreate }: { busy: boolean; error: string | null; admin: boolean; users: UserRecord[]; onCreate: (body: { label?: string; allowed_models?: string[]; user_id?: string }) => void }) {
  const [label, setLabel] = useState('');
  const [models, setModels] = useState('');
  const [owner, setOwner] = useState('');
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onCreate({
      label: label.trim() || undefined,
      allowed_models: models.split(',').map((m) => m.trim()).filter(Boolean),
      user_id: owner || undefined,
    });
    setLabel('');
    setModels('');
  };
  return (
    <form onSubmit={submit} className="mb-2 flex flex-wrap items-center gap-2" data-testid="key-form">
      <input className={INPUT} placeholder="label (e.g. laptop)" aria-label="key label" value={label} onChange={(e) => setLabel(e.target.value)} />
      <input className={cn(INPUT, 'w-56')} placeholder="allowed models, comma-separated (empty = any)" aria-label="allowed models" value={models} onChange={(e) => setModels(e.target.value)} />
      {admin && users.length > 0 && (
        <select className={INPUT} aria-label="key owner" value={owner} onChange={(e) => setOwner(e.target.value)}>
          <option value="">owner: me</option>
          {users.map((u) => (
            <option key={u.id} value={u.id}>owner: {u.username}</option>
          ))}
        </select>
      )}
      <Button type="submit" disabled={busy}>{busy ? 'creating…' : 'create key'}</Button>
      {error && <span className="text-xs text-status-down" data-testid="key-form-error">{error}</span>}
    </form>
  );
}

function KeyEditor({record,users,admin,busy,error,onSave,onCancel}:{record:ApiKeyRecord;users:UserRecord[];admin:boolean;busy:boolean;error:string|null;onSave:(body:UpdateKeyRequest)=>void;onCancel:()=>void}) {
  const [label,setLabel]=useState(record.label??'');
  const [owner,setOwner]=useState(record.user_id);
  const [models,setModels]=useState(record.allowed_models);
  return <Panel raised className="mb-3 space-y-3 p-3" data-testid="key-editor">
    <PanelHeader label="Edit API key" help="Save changes without replacing the token or resetting activity. Only administrators may change the owning user. If another administrator changes this key, discard and reopen to load the new version." actions={<><Button variant="ghost" disabled={busy} onClick={onCancel}>Discard key changes</Button><Button disabled={busy||label.length>128||models.length>64||models.some(model=>!model.trim()||model.length>512)} onClick={()=>onSave({label:label.trim()||null,user_id:owner,allowed_models:models,expected_updated_at_ms:record.updated_at_ms})}>Save key changes</Button></>}><h3 className="text-sm font-semibold">Edit {record.label??'unlabelled key'}</h3></PanelHeader>
    <div className="grid min-w-0 gap-4 md:grid-cols-2">
      <SettingsField name="key label" schema={{kind:'text',label:'Key label'}} value={label} onChange={value=>setLabel(String(value??''))} disabled={busy}/>
      {admin&&<label className="space-y-2 text-sm font-medium">Key owner<select aria-label="Key owner" className="block w-full rounded-md border border-line bg-bg px-3 py-2 text-sm" disabled={busy} value={owner??''} onChange={event=>setOwner(event.target.value||null)}><option value="">No owner</option>{users.map(u=><option key={u.id} value={u.id}>{u.username}</option>)}</select></label>}
      <div className="md:col-span-2"><SettingsField name="allowed models" schema={{kind:'list',label:'Allowed models',help:'Client-facing models or aliases. No entries means any model. Existing custom or currently unavailable model names are preserved.',item:{kind:'text',placeholder:'Model or alias name'}}} value={models} onChange={value=>setModels(Array.isArray(value)?value.map(String):[])} disabled={busy}/></div>
    </div>
    {error&&<p role="alert" className="text-xs text-status-down">{error}</p>}
  </Panel>;
}

function KeyTable({ keys, ownerName, onEdit, onRotate, onRevoke, busy }: { keys: ApiKeyRecord[]; ownerName: (id: string | null) => string; onEdit:(key:ApiKeyRecord)=>void;onRotate:(key:ApiKeyRecord)=>void;onRevoke: (id: string) => void; busy: boolean }) {
  const columns: DataTableColumn<ApiKeyRecord>[] = [
    { id: 'label', label: 'Label', required: true, value:k=>k.label??'',render:k=>k.label??<span className="italic text-text-muted">Unlabelled</span> },
    { id: 'owner', label: 'Owner', value:k=>ownerName(k.user_id),render:k=>ownerName(k.user_id) },
    { id: 'models', label: 'Allowed models', value:k=>k.allowed_models.join(', ')||'Any',render:k=><span className="break-words">{k.allowed_models.join(', ')||'Any'}</span> },
    { id: 'created', label: 'Created', value:k=>k.created_at_ms,render:k=>fmtWhen(k.created_at_ms) },
    { id: 'actions', label: 'Actions',required:true,sortable:false, render:k=><div className="flex flex-wrap gap-1"><Button variant="ghost" disabled={busy} onClick={()=>onEdit(k)} data-testid="key-edit">Edit</Button><Button variant="ghost" disabled={busy} onClick={()=>onRotate(k)} data-testid="key-rotate">Rotate token</Button><Button variant="danger" disabled={busy} onClick={()=>onRevoke(k.id)} data-testid="key-revoke">revoke</Button></div> },
  ];
  return <DataTable id="account-keys" interactive rows={keys} rowKey={k=>k.id} columns={columns} rowTestId="key-row" rowAttributes={k=>({title:k.id})} clientPageSize={25} minWidth={750} mobileColumns={['label','models','actions']} emptyMessage="No keys yet."/>;
}

function UserForm({ busy, error, onCreate }: { busy: boolean; error: string | null; onCreate: (body: { username: string; password: string; is_admin: boolean }) => void }) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [admin, setAdmin] = useState(false);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onCreate({ username: username.trim(), password, is_admin: admin });
    setUsername('');
    setPassword('');
    setAdmin(false);
  };
  return (
    <form onSubmit={submit} className="mb-2 flex flex-wrap items-center gap-2" data-testid="user-form">
      <input className={INPUT} placeholder="username" aria-label="new username" autoComplete="off" value={username} onChange={(e) => setUsername(e.target.value)} />
      <input className={INPUT} type="password" placeholder="password (8+ chars)" aria-label="new password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} />
      <label className="flex items-center gap-1 text-xs text-text-muted">
        <input type="checkbox" checked={admin} onChange={(e) => setAdmin(e.target.checked)} aria-label="administrator" /> admin
      </label>
      <Button type="submit" disabled={busy || username.length === 0 || password.length < 8}>{busy ? 'creating…' : 'create user'}</Button>
      {error && <span className="text-xs text-status-down" data-testid="user-form-error">{error}</span>}
    </form>
  );
}

function UserTable({ users, selfId, keys, onResetPassword, onToggleAdmin, onDelete }: { users: UserRecord[]; selfId: string | null; keys: ApiKeyRecord[]; onResetPassword: (id: string, password: string) => void; onToggleAdmin: (id: string, is_admin: boolean) => void; onDelete: (id: string) => void }) {
  const [resetFor, setResetFor] = useState<string | null>(null);
  const [newPassword, setNewPassword] = useState('');
  if (users.length === 0) return <div className="rounded-md border border-line px-3 py-3 text-xs italic text-text-muted" data-testid="users-empty">No users yet: create the first administrator above (or via the CLI).</div>;
  const columns: DataTableColumn<UserRecord>[] = [
    { id: 'username', label: 'username', width: '20%', required: true, render: (u) => <span className="font-mono">{u.username}{u.id === selfId && <span className="ml-1 text-[9px] text-text-muted">(you)</span>}</span> },
    { id: 'role', label: 'role', width: '10%', render: (u) => <span className={cn('w-fit rounded-sm px-1 text-[9px] uppercase tracking-wide', u.is_admin ? 'bg-accent/15 text-accent' : 'bg-line/40 text-text-muted')}>{u.is_admin ? 'admin' : 'user'}</span> },
    { id: 'keys', label: 'keys', width: '8%', align: 'right', render: (u) => keys.filter((k) => k.user_id === u.id).length },
    { id: 'created', label: 'created', width: '17%', render: (u) => <span className="tabular-nums text-text-muted">{fmtWhen(u.created_at_ms)}</span> },
    { id: 'actions', label: '', width: '45%', required: true, render: (u) => <span className="flex flex-wrap items-center gap-1">
      {resetFor === u.id ? <>
        <input className={INPUT} type="password" placeholder="new password" aria-label={`new password for ${u.username}`} value={newPassword} onChange={(e) => setNewPassword(e.target.value)} />
        <Button className="px-2 py-0.5 text-[11px]" disabled={newPassword.length < 8} onClick={() => { onResetPassword(u.id, newPassword); setResetFor(null); setNewPassword(''); }}>save</Button>
        <Button variant="ghost" className="px-2 py-0.5 text-[11px]" onClick={() => setResetFor(null)}>cancel</Button>
      </> : <>
        <Button variant="ghost" className="px-2 py-0.5 text-[11px]" onClick={() => setResetFor(u.id)} data-testid="user-reset">reset password</Button>
        <Button variant="ghost" className="px-2 py-0.5 text-[11px]" disabled={u.id === selfId} onClick={() => onToggleAdmin(u.id, !u.is_admin)} data-testid="user-toggle-admin">{u.is_admin ? 'make user' : 'make admin'}</Button>
        <Button variant="danger" className="px-2 py-0.5 text-[11px]" disabled={u.id === selfId} onClick={() => onDelete(u.id)} data-testid="user-delete">delete</Button>
      </>}
    </span> },
  ];
  return <div className="rounded-md border border-line"><DataTable id="account-users" rows={users} rowKey={(u) => u.id}
    columns={columns} rowTestId="user-row" rowAttributes={(u) => ({ 'data-admin': u.is_admin ? 'true' : 'false' })}
    clientPageSize={25} minWidth={750} /></div>;
}
