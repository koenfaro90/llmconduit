import {useRef,useState} from 'react';
import type {AuthApiKey,AuthGroup,AuthPolicy,AuthRole,AuthUser,UpdateAuthGroupRequest,UpdateAuthKeyRequest,UpdateAuthPolicyRequest,UpdateAuthRoleRequest,UpdateAuthUserRequest} from '../../api/types';
import {getConnection} from '../../api/connection';
import {Panel,PanelHeader} from '../../components/ui/Panel';
import {Button} from '../../components/ui/Button';
import {SettingsField} from '../settings/SettingsField';
import type {FieldSchema} from '../settings/settingsSchema';
import type {Json} from '../settings/settingsModel';
import {inferenceEndpointOptions,managementPermissionLabels} from './policyEditorModel';

import {weekDays,windowToDraft,windowFromDraft} from './accessEditModel';

export type AccessEditTarget={kind:'user';record:AuthUser}|{kind:'group';record:AuthGroup}|{kind:'role';record:AuthRole}|{kind:'policy';record:AuthPolicy}|{kind:'key';record:AuthApiKey};
type Choices={users:AuthUser[];groups:AuthGroup[];roles:AuthRole[];keys:AuthApiKey[]};
const list=(label:string,help:string,choices?:Array<{value:string;label:string}>):FieldSchema=>({kind:'list',label,help,item:{kind:'text',...(choices?{options:choices.map(c=>c.value),labels:Object.fromEntries(choices.map(c=>[c.value,c.label]))}:{placeholder:'Name or wildcard pattern'})}});
const localDate=(iso:string|null)=>{if(!iso)return '';const date=new Date(iso);return new Date(date.getTime()-date.getTimezoneOffset()*60000).toISOString().slice(0,16);};
function initial(target:AccessEditTarget):Record<string,Json>{
 const data=JSON.parse(JSON.stringify(target.record)) as Record<string,Json>;
 for(const key of ['id','revision','created_at','last_used_at','prefix','member_count','expires_at'])delete data[key];
 if(target.kind==='role')data.bindings=(target.record.bindings??[]).map(b=>`${b.subject_kind}:${b.subject_id}`);
 if(target.kind==='policy'){delete data.models;data.time_windows=target.record.time_windows.map(windowToDraft);}
 if(target.kind==='key')delete data.enabled;
 return data;
}
export function AccessRecordEditor({target,choices,bootstrap,onSaved,onDiscard}:{target:AccessEditTarget;choices:Choices;bootstrap:boolean;onSaved:()=>Promise<unknown>;onDiscard:()=>void}){
 const {client}=getConnection();const [draft,setDraft]=useState(()=>initial(target));const [expiry,setExpiry]=useState(target.kind==='key'?localDate(target.record.expires_at):'');const [busy,setBusy]=useState(false);const [error,setError]=useState('');const fields=useRef<HTMLDivElement>(null);
 const people=choices.users.map(u=>({value:u.id,label:u.display_name+(u.enabled?'':' (disabled)')}));
 const subjects=[...people.map(u=>({value:`principal:${u.value}`,label:`Identity · ${u.label}`})),...choices.groups.map(g=>({value:`group:${g.id}`,label:`Group · ${g.name}`})),...choices.roles.map(r=>({value:`role:${r.id}`,label:`Role · ${r.name}`})),...choices.keys.map(k=>({value:`key:${k.id}`,label:`Key · ${k.name}`}))];
 const enabled:FieldSchema={kind:'boolean',label:'Enabled',default:true,help:'Controls access for new requests. Disabling does not delete history.',readonly:!bootstrap};
 const permissions=list('Management permissions','Dashboard operations granted by enabled policies that target this role or policy.',Object.entries(managementPermissionLabels).map(([value,label])=>({value,label})));
 let schema:FieldSchema;
 switch(target.kind){
 case 'user':schema={kind:'object',fields:{display_name:{kind:'text',label:'Display name',required:true},kind:{kind:'text',label:'Identity kind',required:true,options:['user','service_account'],labels:{user:'Person',service_account:'Service account'},readonly:!bootstrap},enabled}};break;
 case 'group':schema={kind:'object',fields:{name:{kind:'text',label:'Group name',required:true},enabled,members:{...list('Group members','Identities that inherit policies for this group.',people),readonly:!bootstrap}}};break;
 case 'role':schema={kind:'object',fields:{name:{kind:'text',label:'Role name',required:true},enabled,permissions,bindings:{...list('Role assignments','Assign this role to people, services or groups. Enabled policies targeting the role provide its access.',subjects.filter(s=>s.value.startsWith('principal:')||s.value.startsWith('group:'))),readonly:!bootstrap}}};break;
 case 'key':schema={kind:'object',fields:{name:{kind:'text',label:'Key name',required:true},principal_id:{kind:'text',label:'Owning identity',options:people.map(p=>p.value),labels:Object.fromEntries(people.map(p=>[p.value,p.label])),required:true,readonly:!bootstrap}}};break;
 case 'policy':schema={kind:'object',fields:{
  name:{kind:'text',label:'Policy name',required:true},enabled,effect:{kind:'text',label:'Decision',required:true,options:['allow','deny']},
  subjects:list('Applies to','Select the identities, groups, roles or keys governed by this policy.',subjects),
  endpoints:list('API capabilities','No selections means all API capabilities.',[{value:'*',label:'All capabilities'},...inferenceEndpointOptions]),
  requested_models:list('Requested models','Client-facing model or alias names; wildcard patterns are supported. No entries means any.'),
  served_models:list('Served models','Upstream model names or patterns. No entries means any.'),
  providers:list('Providers','Provider names or wildcard patterns. No entries means any provider.'),routes:list('Routes','Routing labels or patterns. No entries means any route.'),
  time_windows:{kind:'list',label:'UTC schedule windows',help:'Add as many windows as needed. No windows means always available; no selected weekdays means every day. Equal start/end times mean the whole day.',item:{kind:'object',label:'Schedule window',fields:{
   days:list('Days of week','Select weekdays, or leave all unchecked for every day.',weekDays.map(day=>({value:day,label:day}))),
   start_time:{kind:'text',label:'Start time (UTC)',inputType:'time',required:true,default:'00:00'},end_time:{kind:'text',label:'End time (UTC)',inputType:'time',required:true,default:'00:00'},
   absolute_start:{kind:'text',label:'Absolute start (UTC)',inputType:'datetime-local',step:0.001,nullable:true},absolute_end:{kind:'text',label:'Absolute end (UTC)',inputType:'datetime-local',step:0.001,nullable:true},
  }}},
  max_concurrent_sessions:{kind:'number',label:'Concurrent sessions',min:1,nullable:true,help:'Leave unset for no policy limit.'},max_daily_session_starts:{kind:'number',label:'Daily session starts',min:1,nullable:true,help:'Leave unset for no policy limit.'},management_permissions:permissions,
 }};if(!bootstrap&&schema.fields)for(const [name,field]of Object.entries(schema.fields))if(name!=='name')field.readonly=true;break;
 }
 async function save(){
  if(fields.current!.querySelector('[aria-label="Finish entry editing"]')){setError('Update or cancel the open schedule entry before saving.');return;}
  if([...fields.current!.querySelectorAll<HTMLInputElement>('input[aria-label^="New "]')].some(input=>input.value.trim())){setError('Add or clear the unfinished list value before saving.');return;}
  if([...fields.current!.querySelectorAll<HTMLInputElement>('input,select,textarea')].some(input=>!input.checkValidity())){fields.current!.querySelector<HTMLInputElement>(':invalid')?.reportValidity();return;}
  if(target.record.revision===undefined){setError('Refresh Access before editing this record.');return;}
  setBusy(true);setError('');const body={...draft,expected_revision:target.record.revision};
  try{
   switch(target.kind){
    case 'user':await client.updateAuthUser(target.record.id,body as unknown as UpdateAuthUserRequest);break;
    case 'group':await client.updateAuthGroup(target.record.id,body as unknown as UpdateAuthGroupRequest);break;
    case 'role':await client.updateAuthRole(target.record.id,{...body,bindings:(draft.bindings as string[]).map(s=>{const [subject_kind,...id]=s.split(':');return {subject_kind,subject_id:id.join(':')};})} as unknown as UpdateAuthRoleRequest);break;
    case 'policy':await client.updateAuthPolicy(target.record.id,{models:[],max_concurrent_sessions:null,max_daily_session_starts:null,...body,time_windows:(draft.time_windows as Record<string,Json>[]??[]).map(windowFromDraft)} as unknown as UpdateAuthPolicyRequest);break;
    case 'key':await client.updateAuthKey(target.record.id,{...body,expires_at:expiry===localDate(target.record.expires_at)?target.record.expires_at:expiry?new Date(expiry).toISOString():null} as unknown as UpdateAuthKeyRequest);break;
   }
   await onSaved();
  }catch(e){setError(e instanceof Error?e.message:'Save failed');}finally{setBusy(false);}
 }
 return <Panel raised className="mb-4 space-y-4 p-4" data-testid="access-record-editor"><PanelHeader label="Edit Access record" help="Edit the existing record in place. IDs, credentials and historical activity are retained. Changes are validated and published live. A concurrent Access change rejects this draft instead of overwriting it." actions={<><Button variant="ghost" disabled={busy} onClick={onDiscard}>Discard Access changes</Button><Button disabled={busy} onClick={()=>void save()}>Save Access changes</Button></>}><h2 className="font-semibold">Edit {target.kind==='user'?target.record.display_name:target.record.name}</h2></PanelHeader>
 {!bootstrap&&<p className="text-xs text-text-muted">Your delegated role permits metadata edits. Changing grants, membership, ownership or enabled state requires a bootstrap administrator.</p>}
 <div ref={fields}><SettingsField schema={schema} value={draft} name={`edit ${target.kind}`} onChange={value=>setDraft(value as Record<string,Json>)} disabled={busy}/>
 {target.kind==='key'&&<label className="mt-4 block text-sm">Expires at (local time)<input aria-label="Key expiration" type="datetime-local" value={expiry} disabled={busy||!bootstrap} onChange={event=>setExpiry(event.target.value)} className="mt-2 block rounded border border-line bg-bg px-3 py-2"/><span className="mt-1 block text-xs text-text-muted">Leave blank for no expiry. Revoked keys remain revoked.</span></label>}</div>
 {error&&<p role="alert" className="text-xs text-status-down">{error}</p>}
 </Panel>;
}
