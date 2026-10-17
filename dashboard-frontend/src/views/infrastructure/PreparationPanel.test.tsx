import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import {cleanup,fireEvent,screen,waitFor} from '@testing-library/react';
import {renderWithQuery,resetWorld} from '../../components/testHarness';
import {getConnection} from '../../api/connection';
import {authStore} from '../../store/authStore';
import type {ManagementState,MeshNode} from '../../api/types';
import {PreparationPanel} from './PreparationPanel';
import {isPreparationRun,type PreparationRun} from './preparationModel';
const digest='a'.repeat(64),revision='b'.repeat(40),image=`ghcr.io/example/image@sha256:${'c'.repeat(64)}`;
const state:ManagementState={recipes:{deepseek:[{digest,revision:1,description:'Pinned target',model_spec:{image,metadata:{checkpoints:[{repository:'owner/model',revision}]}},created_at_ms:1}]},deployments:{}};
const nodes=['gpu03','gpu01'].map(name=>({endpoint_id:name,enabled:true,managed_deployments:true,label:name,host_snapshot:{hostname:name,sampled_at:new Date().toISOString(),gpus:[]}} as unknown as MeshNode));
function run():PreparationRun{return {id:'01234567-0123-0123-0123-012345678901',endpoint_id:'gpu01',recipe_id:'deepseek',revision_digest:digest,rank:0,image,checkpoints:[{repository:'owner/model',revision}],created_at_ms:1000,cancel_requested:false,report:null,error:null};}
beforeEach(()=>{resetWorld({mock:true});const {client}=getConnection();authStore.getState().setMutationsEnabled(true);vi.spyOn(client,'preparations').mockResolvedValue([]);});
afterEach(()=>{cleanup();vi.restoreAllMocks();});
it('prepares pinned recipes on sorted hosts without invoking inference lifecycle',async()=>{
 const {client}=getConnection();const start=vi.spyOn(client,'startPreparation').mockResolvedValue(run());const deploy=vi.spyOn(client,'deployGroup');renderWithQuery(<PreparationPanel nodes={nodes} state={state}/>);
 fireEvent.change(screen.getByLabelText('Preparation recipe'),{target:{value:`deepseek:${digest}`}});const hosts=screen.getByLabelText('Preparation hosts') as HTMLSelectElement;expect(Array.from(hosts.options,o=>o.text)).toEqual(['gpu01','gpu03']);for(const option of hosts.options)option.selected=true;fireEvent.change(hosts);fireEvent.click(screen.getByRole('button',{name:'Prepare'}));
 await waitFor(()=>expect(start).toHaveBeenCalledTimes(2));expect(start).toHaveBeenCalledWith(expect.objectContaining({endpoint_id:'gpu01',recipe_id:'deepseek',revision_digest:digest,rank:0}));expect(start).toHaveBeenCalledWith(expect.objectContaining({endpoint_id:'gpu03',revision_digest:digest}));expect(deploy).not.toHaveBeenCalled();expect(screen.getByRole('link',{name:'owner/model'})).toHaveAttribute('href',`https://huggingface.co/owner/model/tree/${revision}`);
});
it('shows verified bytes separately from network transfer and cancels the durable job',async()=>{
 const r=run();r.report={id:r.id,recipe_digest:digest,image,checkpoints:r.checkpoints,status:'running',phase:'downloading_checkpoints',message:'',started_at_ms:1000,updated_at_ms:3000,total_bytes:10*1024**3,completed_bytes:3*1024**3,downloaded_bytes:5*1024**3,verified_files:3,total_files:10,current_file:'weights.safetensors',sequence:4};const {client}=getConnection();vi.mocked(client.preparations).mockResolvedValue([r]);const cancel=vi.spyOn(client,'cancelPreparation').mockResolvedValue({...r,cancel_requested:true});renderWithQuery(<PreparationPanel nodes={nodes} state={state}/>);
 const progress=await screen.findByRole('progressbar');expect(progress).toHaveAttribute('value',String(3*1024**3));expect(screen.getByText('3.0 GiB / 10.0 GiB · 3/10 files')).toBeInTheDocument();fireEvent.click(screen.getByRole('button',{name:'Cancel'}));await waitFor(()=>expect(cancel).toHaveBeenCalledWith(r.id));
});
it('rejects unpinned or inconsistent receipts',()=>{const r=run();expect(isPreparationRun(r)).toBe(true);expect(isPreparationRun({...r,checkpoints:[{repository:'owner/model',revision:'main'}]})).toBe(false);expect(isPreparationRun({...r,report:{id:r.id,recipe_digest:'different',image,status:'complete'}})).toBe(false);});
