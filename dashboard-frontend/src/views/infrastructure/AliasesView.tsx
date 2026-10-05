import { useState, type FormEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { getConnection } from '../../api/connection';
import { useAuth } from '../../store/hooks';
import { Button } from '../../components/ui/Button';
import { Panel } from '../../components/ui/Panel';
import { DataTable } from '../../components/ui/DataTable';
import { StatusIcon } from '../../components/ui/StatusIcon';
import type { AliasMapping, AliasView, AliasesResponse } from './aliasModel';
const KEY = ['model-aliases'];
const INPUT = 'w-full min-w-0 rounded border border-line bg-bg p-2 text-sm outline-none focus:border-accent';
interface Draft { revision: number; aliases: AliasMapping[]; mapping: AliasMapping; removing: boolean }
export function AliasesView() {
  const { client } = getConnection();
  const cache = useQueryClient();
  const mutationsEnabled = useAuth(s => s.mutationsEnabled);
  const user = useAuth(s => s.user);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [query, setQuery] = useState('');
  const [notice, setNotice] = useState('');
  const data = useQuery({ queryKey: KEY, queryFn: () => client.aliases(), retry: false, refetchInterval: draft ? false : 15_000 });
  const canEdit = mutationsEnabled && (!user || user.is_admin) && !!data.data?.persistent;
  const save = useMutation({ mutationFn: ({ revision, aliases }: { revision: number; aliases: AliasMapping[] }) => client.saveAliases(revision, aliases), onSuccess: (value: AliasesResponse) => {
    cache.setQueryData(KEY, value); setDraft(null); setNotice('Alias changes are live.');
    void cache.invalidateQueries({ predicate: q => q.queryKey[0] !== KEY[0] });
  } });
  function edit(mapping?: AliasView, removing = false) {
    if (!data.data) return;
    save.reset(); setNotice('');
    setDraft({ revision: data.data.revision, aliases: data.data.aliases.map(({ id, name, profiles }) => ({ id, name, profiles: [...profiles] })), mapping: mapping ? { id: mapping.id, name: mapping.name, profiles: [...mapping.profiles] } : { id: crypto.randomUUID(), name: '', profiles: [''] }, removing });
  }
  function submit(e: FormEvent) {
    e.preventDefault(); if (!draft || !canEdit) return;
    const mapping = { ...draft.mapping, name: draft.mapping.name.trim() };
    const aliases = draft.aliases.filter(a => a.id !== mapping.id);
    if (!draft.removing) aliases.push(mapping);
    save.mutate({ revision: draft.revision, aliases });
  }
  const target = (id: string) => data.data?.targets.find(t => t.id === id);
  const description = (row: AliasView) => row.profiles.map(id => target(id)?.name ?? 'Missing target').join(' → ');
  const rows = (data.data?.aliases ?? []).filter(a => `${a.name} ${description(a)}`.toLowerCase().includes(query.toLowerCase()));
  return <div className="min-w-0 flex-1 overflow-auto p-3 sm:p-4 space-y-4" data-testid="aliases-view">
    <header className="flex flex-wrap items-start justify-between gap-3"><div><h1 className="text-lg font-semibold">Aliases</h1><p className="text-xs text-text-muted">Stable client names for your models · changes apply immediately</p></div><Button disabled={!canEdit || !!draft} onClick={() => edit()}>Add alias</Button></header>
    <Panel className="p-3"><p className="text-sm">Clients request an alias such as <code className="text-accent">assistant-default</code>. Its primary model supplies request defaults; the ordered targets supply backend failover.</p><p className="mt-2 text-xs text-text-muted">Mappings persist across restarts and config syncs. In-flight requests retain their original mapping. Model and API-key policies still apply; renaming an alias does not rewrite those policies.</p></Panel>
    {data.isLoading && <p role="status">Loading aliases…</p>}
    {data.error && <Panel className="p-3"><p role="alert">{String(data.error)}</p><Button variant="ghost" onClick={() => void data.refetch()}>Retry</Button></Panel>}
    {data.data && !canEdit && <p className="text-xs text-text-muted">Read-only: editing requires an administrator, enabled dashboard mutations and SQL persistence.</p>}
    {notice && <p role="status" className="text-sm text-accent">{notice}</p>}
    {data.data && <DataTable interactive id="model-aliases" tableTestId="model-aliases" rows={rows} rowKey={r => r.id} mobileColumns={['alias', 'targets', 'actions']} minWidth={800} filterBar={<input aria-label="Search aliases" className={INPUT + ' sm:max-w-xs'} placeholder="Find an alias or model…" value={query} onChange={e => setQuery(e.target.value)}/>} columns={[
      { id: 'alias', label: 'Alias', required: true, value: r => r.name, render: r => <code className="text-accent">{r.name}</code> },
      { id: 'targets', label: 'Primary / fallback models', value: description, render: r => <div className="space-y-1">{r.profiles.map((id, i) => <div key={id}><span className="mr-2 text-xs text-text-muted">{i === 0 ? 'Primary' : `Fallback ${i}`}</span>{target(id)?.name ?? <span className="text-danger">Missing target</span>}</div>)}</div> },
      { id: 'upstream', label: 'Upstream model IDs', value: r => r.profiles.flatMap(id => target(id)?.upstream_models ?? []).join(', '), render: r => <span className="break-all text-xs">{r.profiles.flatMap(id => target(id)?.upstream_models ?? []).join(' · ') || '—'}</span> },
      { id: 'backends', label: 'Backends', value: r => [...new Set(r.profiles.flatMap(id => target(id)?.backends ?? []))].join(', '), render: r => <span className="text-xs">{[...new Set(r.profiles.flatMap(id => target(id)?.backends ?? []))].join(' · ') || '—'}</span> },
      { id: 'state', label: 'State', value: r => r.error ? 'Needs review' : 'Configured', render: r => <span title={r.error ?? 'Configured mapping; backend availability is shown under Models'}><StatusIcon status={r.error ? 'error' : 'configured'}/>{r.error && <span className="ml-1 text-xs">Needs review</span>}</span> },
      { id: 'actions', label: 'Actions', sortable: false, render: r => <div className="flex gap-1"><Button variant="ghost" disabled={!canEdit || !!draft} aria-label={`Edit ${r.name}`} onClick={() => edit(r)}>Edit</Button><Button variant="ghost" disabled={!canEdit || !!draft} aria-label={`Delete ${r.name}`} onClick={() => edit(r, true)}>Delete</Button></div> },
    ]} emptyMessage={query ? 'No matching aliases.' : 'No aliases configured. Add a stable name for a model.'}/>}
    {draft && <Panel className="p-4"><form onSubmit={submit}><fieldset disabled={save.isPending} className="min-w-0 space-y-4">
      <h2 className="text-sm font-semibold">{draft.removing ? `Delete ${draft.mapping.name}?` : draft.aliases.some(a => a.id === draft.mapping.id) ? 'Edit alias' : 'Add alias'}</h2>
      {draft.removing ? <p className="text-sm">New requests for this alias will be rejected. Requests already in progress will finish with their original target.</p> : <>
        <label className="block text-xs sm:max-w-lg">Alias name<input aria-label="Alias name" autoFocus required maxLength={128} pattern={'[^\\s]+'} className={INPUT + ' mt-1'} value={draft.mapping.name} onChange={e => setDraft({ ...draft, mapping: { ...draft.mapping, name: e.target.value } })}/></label>
        <div className="space-y-2 sm:max-w-3xl">{draft.mapping.profiles.map((id, i) => <div key={i} className="flex flex-wrap items-end gap-2"><label className="min-w-0 flex-1 text-xs">{i === 0 ? 'Primary model' : `Fallback ${i}`}<select required aria-label={i === 0 ? 'Primary model' : `Fallback ${i}`} className={INPUT + ' mt-1'} value={id} onChange={e => setDraft({ ...draft, mapping: { ...draft.mapping, profiles: draft.mapping.profiles.map((p, j) => j === i ? e.target.value : p) } })}><option value="">Select a model…</option>{id && !target(id) && <option value={id}>Missing target — choose a model</option>}{data.data?.targets.map(t => <option key={t.id} value={t.id} disabled={!t.available || draft.mapping.profiles.some((p, j) => j !== i && p === t.id)}>{t.name}{t.available ? '' : ' (no enabled backend)'}</option>)}</select></label>{i > 0 && <><Button type="button" variant="ghost" aria-label={`Move fallback ${i} up`} onClick={() => { const profiles = [...draft.mapping.profiles]; [profiles[i - 1], profiles[i]] = [profiles[i]!, profiles[i - 1]!]; setDraft({ ...draft, mapping: { ...draft.mapping, profiles } }); }}>↑</Button><Button type="button" variant="ghost" aria-label={`Remove fallback ${i}`} onClick={() => setDraft({ ...draft, mapping: { ...draft.mapping, profiles: draft.mapping.profiles.filter((_, j) => j !== i) } })}>Remove</Button></>}</div>)}</div>
        <Button type="button" variant="ghost" disabled={draft.mapping.profiles.length >= 16 || draft.mapping.profiles.length >= (data.data?.targets.filter(t => t.available).length ?? 0)} onClick={() => setDraft({ ...draft, mapping: { ...draft.mapping, profiles: [...draft.mapping.profiles, ''] } })}>Add fallback model</Button>
      </>}
      {save.error && <p role="alert" className="text-sm">{String(save.error)}{String(save.error).includes('409') && ' Aliases changed. Cancel and reload before trying again; your draft has been kept.'}</p>}
      <div className="flex flex-wrap gap-2"><Button type="submit" variant={draft.removing ? "danger" : "default"} disabled={!canEdit || save.isPending || (!draft.removing && (!draft.mapping.name.trim() || draft.mapping.profiles.some(id => !id || !target(id)) || new Set(draft.mapping.profiles).size !== draft.mapping.profiles.length))}>{save.isPending ? 'Saving…' : draft.removing ? 'Delete alias' : 'Save alias'}</Button><Button type="button" variant="ghost" disabled={save.isPending} onClick={() => { setDraft(null); save.reset(); void data.refetch(); }}>Cancel</Button></div>
    </fieldset></form></Panel>}
  </div>;
}
