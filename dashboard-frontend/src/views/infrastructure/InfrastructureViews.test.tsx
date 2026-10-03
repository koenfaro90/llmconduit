import {afterEach,beforeEach,it,expect,vi} from 'vitest';
import {cleanup,screen,waitFor,fireEvent} from '@testing-library/react';
import {renderWithQuery,resetWorld} from '../../components/testHarness';
import {getConnection} from '../../api/connection';
import {RecipeManagement} from '../providers/RecipeManagement';
import {HostsView} from './HostsView';import {ModelsView} from './ModelsView';import {RecipesView} from './RecipesView';import {DeploymentView} from './DeploymentView';
beforeEach(()=>{resetWorld({mock:true});vi.spyOn(getConnection().client,'management').mockResolvedValue({recipes:{model:[{revision:1,digest:'pinned',description:'Inference',model_spec:{image:'sha256:abc',command:['--served-model-name','actual']},created_at_ms:1}]},deployments:{dep:{endpoint_id:'node',recipe_id:'model',revision_digest:'pinned',generation:1,desired_state:'ready',updated_at_ms:1,observed_at_ms:Date.now(),observed:{status:{phase:'ready'}},error:null}}});vi.spyOn(getConnection().client,'modelActivity').mockResolvedValue([{model:'actual',backend:'backend',last_request_at_ms:Date.now(),requests:[2,4,6,8,10]}]);});
afterEach(()=>{cleanup();vi.restoreAllMocks();});
it('hosts is informational and renders shared charts',async()=>{renderWithQuery(<HostsView/>);await screen.findByTestId('hosts-view');await waitFor(()=>expect(screen.getAllByRole('img').length).toBeGreaterThan(0));expect(screen.queryByRole('button',{name:'Stop'})).toBeNull();expect(screen.getByLabelText('Host history range')).toHaveValue('hour');expect(screen.getByText(/Stored Fleet history/)).toBeInTheDocument();});
it('models shows all SQL request windows and pinned configuration',async()=>{renderWithQuery(<ModelsView/>);const table=await screen.findByTestId('running-models');await waitFor(()=>expect(table).toHaveTextContent('actual'));expect(table).toHaveTextContent('24 hours');expect(table).toHaveTextContent('10');fireEvent.click(screen.getByRole('button',{name:'Review'}));expect(screen.getByText(/Image: sha256:abc/)).toBeInTheDocument();expect(screen.queryByRole('button',{name:'Deploy'})).toBeNull();});
it('recipes has the editor without placement controls',async()=>{renderWithQuery(<RecipesView/>);await screen.findByTestId('recipes-view');expect(screen.getByRole('button',{name:'New JSON recipe'})).toBeInTheDocument();expect(screen.queryByLabelText('Deployment host')).toBeNull();});
it('deployment has placement controls without the editor',async()=>{renderWithQuery(<DeploymentView/>);await screen.findByTestId('deployment-view');expect(screen.getByLabelText('Deployment hosts')).toBeInTheDocument();expect(screen.queryByLabelText('Deployment ID')).toBeNull();expect(screen.queryByLabelText('Recipe specification')).toBeNull();});

it('aggregates replicas once and expands every host without duplicating request counts',async()=>{
 const client=getConnection().client;
 const base=await client.management();const dep=base.deployments.dep!;
 vi.mocked(client.management).mockResolvedValue({...base,deployments:{dep,replica:{...dep,endpoint_id:'node-two'}}});
 renderWithQuery(<ModelsView/>);await waitFor(()=>expect(screen.getByTestId('running-models')).toHaveTextContent('2 hosts'));
 fireEvent.click(screen.getByText('actual'));expect(screen.getAllByTestId('running-models-expanded-row')).toHaveLength(2);
 expect(screen.getByTestId('running-models').querySelectorAll('tbody > tr')).toHaveLength(3);
});

it('deploys an ordered two-host recipe with generated identifiers and exposes pinned revisions',async()=>{
 const client=getConnection().client;const spec={distributed:[{image:'pinned'},{image:'pinned'}]};
 vi.mocked(client.management).mockResolvedValue({recipes:{qwen:[{revision:1,digest:'old',description:'tested',model_spec:spec,created_at_ms:1},{revision:2,digest:'new',description:'new',model_spec:spec,created_at_ms:2}]},deployments:{}});
 const deploy=vi.spyOn(client,'deployGroup').mockResolvedValue({group_id:'generated',deployments:{}});
 renderWithQuery(<RecipeManagement mode="deployment" mutationsEnabled nodes={[{endpoint_id:'one',label:'Spark one',enabled:true,joined_at_ms:1,managed_deployments:true},{endpoint_id:'two',label:'Spark two',enabled:true,joined_at_ms:1,managed_deployments:true}]}/>);
 await screen.findByRole('option',{name:'qwen · revision 1'});
 fireEvent.change(screen.getByLabelText('Recipe revision'),{target:{value:'qwen:old'}});
 fireEvent.click(screen.getByRole('checkbox',{name:/Spark two/}));expect(screen.getByRole('button',{name:'Deploy'})).toBeDisabled();
 fireEvent.click(screen.getByRole('checkbox',{name:/Spark one/}));
 fireEvent.change(screen.getByLabelText('Address two'),{target:{value:'192.168.37.108'}});fireEvent.change(screen.getByLabelText('Address one'),{target:{value:'192.168.37.107'}});
 fireEvent.click(screen.getByRole('button',{name:'Deploy'}));
 await waitFor(()=>expect(deploy).toHaveBeenCalledWith({recipe_id:'qwen',revision_digest:'old',hosts:[{endpoint_id:'two',address:'192.168.37.108'},{endpoint_id:'one',address:'192.168.37.107'}],desired_state:'ready'}));
 expect(screen.queryByLabelText('Deployment ID')).toBeNull();
});
