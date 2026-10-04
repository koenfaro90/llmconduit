import {afterEach,beforeEach,it,expect,vi} from 'vitest';
import {cleanup,screen,waitFor,fireEvent} from '@testing-library/react';
import {renderWithQuery,resetWorld} from '../../components/testHarness';
import {getConnection} from '../../api/connection';
import type {MeshNode,FleetHostSnapshot} from '../../api/types';
import {RecipeManagement} from '../providers/RecipeManagement';
import {HostsView} from './HostsView';import {ModelsView} from './ModelsView';import {RecipesView} from './RecipesView';import {DeploymentView} from './DeploymentView';
beforeEach(()=>{resetWorld({mock:true});vi.spyOn(getConnection().client,'management').mockResolvedValue({recipes:{model:[{revision:1,digest:'pinned',description:'Inference',model_spec:{image:'sha256:abc',command:['--served-model-name','actual']},created_at_ms:1}]},deployments:{dep:{endpoint_id:'node',recipe_id:'model',revision_digest:'pinned',generation:1,desired_state:'ready',updated_at_ms:1,observed_at_ms:Date.now(),observed:{status:{phase:'ready'}},error:null}}});vi.spyOn(getConnection().client,'modelActivity').mockResolvedValue([{model:'actual',backend:'backend',last_request_at_ms:Date.now(),requests:[2,4,6,8,10]}]);});
afterEach(()=>{cleanup();vi.restoreAllMocks();});
it('hosts is informational and renders shared charts',async()=>{renderWithQuery(<HostsView/>);await screen.findByTestId('hosts-view');await waitFor(()=>expect(screen.getAllByRole('img').length).toBeGreaterThan(0));expect(screen.queryByRole('button',{name:'Stop'})).toBeNull();expect(screen.getByLabelText('Host history range')).toHaveValue('hour');expect(screen.getByText(/Stored Fleet history/)).toBeInTheDocument();});
it('models shows all SQL request windows and pinned configuration',async()=>{renderWithQuery(<ModelsView/>);const table=await screen.findByTestId('running-models');await waitFor(()=>expect(table).toHaveTextContent('actual'));expect(table).toHaveTextContent('24 hours');expect(table).toHaveTextContent('10');fireEvent.click(screen.getByRole('button',{name:'Review'}));expect(screen.getByText(/Image: sha256:abc/)).toBeInTheDocument();expect(screen.queryByRole('button',{name:'Deploy'})).toBeNull();});
it('recipes has the editor without placement controls',async()=>{renderWithQuery(<RecipesView/>);await screen.findByTestId('recipes-view');expect(screen.getByRole('button',{name:'New JSON recipe'})).toBeInTheDocument();expect(screen.queryByLabelText('Deployment host')).toBeNull();});
it('deployment has placement controls without the editor',async()=>{renderWithQuery(<DeploymentView/>);await screen.findByTestId('deployment-view');expect(screen.queryByLabelText('Deployment hosts')).toBeNull();expect(screen.getByLabelText('Recipe revision')).toBeInTheDocument();expect(screen.queryByLabelText('Deployment ID')).toBeNull();expect(screen.queryByLabelText('Recipe specification')).toBeNull();});

it('aggregates replicas once and expands every host without duplicating request counts',async()=>{
 const client=getConnection().client;
 const base=await client.management();const dep=base.deployments.dep!;
 vi.mocked(client.management).mockResolvedValue({...base,deployments:{dep,replica:{...dep,endpoint_id:'node-two'}}});
 renderWithQuery(<ModelsView/>);await waitFor(()=>expect(screen.getByTestId('running-models')).toHaveTextContent('2 hosts'));
 fireEvent.click(screen.getByText('actual'));expect(screen.getAllByTestId('running-models-expanded-row')).toHaveLength(2);
 expect(screen.getByTestId('running-models').querySelectorAll('tbody > tr')).toHaveLength(3);
});

it('chooses different physical devices for each host and requires the recipe GPU count',async()=>{
 const client=getConnection().client;
 vi.mocked(client.management).mockResolvedValue({recipes:{model:[{revision:1,digest:'pinned',description:'two GPUs',model_spec:{image:'pinned',gpus:'device=0,1',memory_limit:'4g'},created_at_ms:1}]},deployments:{}});
 const deploy=vi.spyOn(client,'deployGroup').mockResolvedValue({group_id:'generated',deployments:{}});
 const nodes=['one','two'].map((id,index)=>({endpoint_id:id,label:id,enabled:true,managed_deployments:true,host_snapshot:{sampled_at:new Date().toISOString(),hostname:id,gpus:Array.from({length:4},(_,i)=>({index:i+index*4,name:'NVIDIA RTX PRO 6000 Blackwell',unified_memory:false,memory_total_mib:96000,memory_used_mib:0})),managed_capacity:{memory_budget_mib:128000,memory_reserved_mib:0,gpu_sharing_enabled:true,gpus:Array.from({length:4},(_,i)=>({index:i+index*4,total_mib:96000,budget_mib:95000,reserved_mib:0,unified_memory:false}))}}} as MeshNode));
 renderWithQuery(<RecipeManagement mode="deployment" mutationsEnabled nodes={nodes}/>);
 await screen.findByRole('option',{name:'model · revision 1'});fireEvent.change(screen.getByLabelText('Recipe revision'),{target:{value:'model:pinned'}});
 fireEvent.click(screen.getByLabelText('GPU 1 on one'));expect(screen.getByLabelText('Deploy to one')).toBeDisabled();
 fireEvent.click(screen.getByLabelText('GPU 3 on one'));fireEvent.click(screen.getByLabelText('Deploy to one'));
 fireEvent.click(screen.getByLabelText('GPU 4 on two'));fireEvent.click(screen.getByLabelText('GPU 6 on two'));fireEvent.click(screen.getByLabelText('Deploy to two'));
 fireEvent.click(screen.getByRole('button',{name:'Deploy'}));
 await waitFor(()=>expect(deploy).toHaveBeenCalledWith(expect.objectContaining({hosts:[{endpoint_id:'one',address:'one',gpus:[0,3]},{endpoint_id:'two',address:'two',gpus:[5,6]}]})));
});

it('deploys an ordered two-host recipe with generated identifiers and exposes pinned revisions',async()=>{
 const client=getConnection().client;const spec={distributed:[{image:'pinned'},{image:'pinned'}]};
 vi.mocked(client.management).mockResolvedValue({recipes:{qwen:[{revision:1,digest:'old',description:'tested',model_spec:spec,created_at_ms:1},{revision:2,digest:'new',description:'new',model_spec:spec,created_at_ms:2}]},deployments:{}});
 const deploy=vi.spyOn(client,'deployGroup').mockResolvedValue({group_id:'generated',deployments:{}});
 renderWithQuery(<RecipeManagement mode="deployment" mutationsEnabled nodes={['one','two'].map(id=>({endpoint_id:id,label:`Spark ${id}`,enabled:true,joined_at_ms:1,managed_deployments:true,host_snapshot:{schema_version:1,sampled_at:new Date().toISOString(),hostname:id,architecture:'arm64',uptime_seconds:1,cpu:null,memory:{total_bytes:100,used_bytes:40,available_bytes:60},disk:null,network:[],containers:[],errors:[],gpus:[{index:0,uuid:'gpu',name:'GB10',unified_memory:true,memory_total_mib:null,memory_used_mib:null,utilization_percent:20,temperature_celsius:40,power_watts:null}]} as FleetHostSnapshot} as MeshNode))}/>);
 await screen.findByRole('option',{name:'qwen · revision 1'});
 fireEvent.change(screen.getByLabelText('Recipe revision'),{target:{value:'qwen:old'}});
 fireEvent.click(screen.getByRole('checkbox',{name:/two/}));expect(screen.getByRole('button',{name:'Deploy'})).toBeDisabled();
 fireEvent.click(screen.getByRole('checkbox',{name:/one/}));
 fireEvent.change(screen.getByLabelText('Address two'),{target:{value:'192.168.37.108'}});fireEvent.change(screen.getByLabelText('Address one'),{target:{value:'192.168.37.107'}});
 fireEvent.click(screen.getByRole('button',{name:'Deploy'}));
 await waitFor(()=>expect(deploy).toHaveBeenCalledWith({recipe_id:'qwen',revision_digest:'old',hosts:[{endpoint_id:'two',address:'192.168.37.108'},{endpoint_id:'one',address:'192.168.37.107'}],desired_state:'ready'}));
 expect(screen.queryByLabelText('Deployment ID')).toBeNull();
});

it('shows the largest compatible memory peak alongside newer ordinary smoke evidence',async()=>{
 const client=getConnection().client;
 const spec={image:'pinned',gpus:'device=0',memory_limit:'28g',gpu_memory:{mode:'shared',mib_per_device:17408,fraction:0}};
 vi.mocked(client.management).mockResolvedValue({recipes:{model:[
  {revision:1,digest:'family',description:'family',created_at_ms:1,model_spec:{...spec,metadata:{gpu_families:['spark']},sizes:{small:{overrides:{memory_limit:'28g'}}}}},
  {revision:2,digest:'old',description:'small',created_at_ms:2,model_spec:{...spec,metadata:{size_name:'small',size_source_digest:'family'}}},
  {revision:3,digest:'new',description:'small',created_at_ms:3,model_spec:{...spec,metadata:{size_name:'small',size_source_digest:'family'}}},
 ]},deployments:{}});
 const base={recipe_id:'model',endpoint_id:'host',kind:'smoke'};
 vi.spyOn(client,'recipeEvidence').mockResolvedValue([
  {...base,id:'ordinary',revision_digest:'new',recorded_at_ms:4,data:{passed_requests:2}},
  {...base,id:'high',revision_digest:'new',recorded_at_ms:3,data:{memory_profile:{gpu_peak_mib:{0:12288},unified_peak_mib:24576}}},
  {...base,id:'low',revision_digest:'old',recorded_at_ms:2,data:{memory_profile:{gpu_peak_mib:{0:11264},unified_peak_mib:20480}}},
 ]);
 renderWithQuery(<RecipesView/>);
 await waitFor(()=>expect(screen.getByTestId('recipe-revisions')).toHaveTextContent('Measured peak: 12.0 GiB GPU · 24.0 GiB unified'));
 expect(screen.getByRole('button',{name:'Review small evidence'})).toBeInTheDocument();
});

it('plots selected host history by physical GPU and can hide one device independently',async()=>{
 const client=getConnection().client;const mesh=await client.mesh();
 const sample:FleetHostSnapshot={schema_version:1,sampled_at:new Date().toISOString(),hostname:'gpu-test',architecture:'amd64',uptime_seconds:1,cpu:null,memory:null,disk:null,network:[],containers:[],errors:[],gpus:[0,1].map(index=>({index,uuid:`physical-${index}`,name:'RTX',unified_memory:false,memory_total_mib:96000,memory_used_mib:1000,utilization_percent:index?70:20,temperature_celsius:45,power_watts:100,fan_speed_percent:50}))};
 vi.spyOn(client,'mesh').mockResolvedValue({...mesh,nodes:[{endpoint_id:'gpu-host',enabled:true,joined_at_ms:1,host_snapshot:sample}]});
 vi.spyOn(client,'hostHistory').mockResolvedValue({samples:[sample],bucket_ms:15000});
 const previous=window.location.hash;window.location.hash='#/hosts/gpu-host';
 try {
  renderWithQuery(<HostsView/>);
  const gpu0=await screen.findByRole('checkbox',{name:'GPU 0 RTX'});await screen.findByRole('checkbox',{name:'GPU 1 RTX'});
  await waitFor(()=>expect(screen.getByRole('img',{name:/GPU 0 · GPU utilization.*GPU 1 · GPU utilization/})).toBeInTheDocument());
  fireEvent.click(gpu0);expect(screen.getByRole('img',{name:/GPU 1 · GPU utilization/})).toBeInTheDocument();expect(screen.queryByRole('img',{name:/GPU 0 · GPU utilization/})).toBeNull();
  fireEvent.click(screen.getByRole('checkbox',{name:'GPU fan speed %'}));expect(screen.getByRole('img',{name:/GPU 1 · GPU fan speed/})).toBeInTheDocument();
 }finally{window.location.hash=previous;}
});
