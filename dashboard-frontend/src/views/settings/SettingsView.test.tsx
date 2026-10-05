import {beforeEach,afterEach,it,expect,vi} from 'vitest';
import {act,cleanup,fireEvent,screen,waitFor} from '@testing-library/react';
import {renderWithQuery,resetWorld} from '../../components/testHarness';
import {getConnection} from '../../api/connection';
import {authStore} from '../../store/authStore';
import {ConfigurationView} from './SettingsView';
import {changedSettings,isHostSettings,isSettingsView,overridesFor,SECRET} from './settingsModel';
beforeEach(()=>resetWorld({mock:true}));afterEach(()=>{cleanup();vi.restoreAllMocks()});
it('stages a field edit, removes its override and explicitly applies the revision',async()=>{
 const client=getConnection().client;const save=vi.spyOn(client,'stageSettings');const apply=vi.spyOn(client,'applySettings');renderWithQuery(<ConfigurationView scope="gateway"/>);
 await screen.findByLabelText('Request timeout');fireEvent.change(screen.getByLabelText('Request timeout'),{target:{value:'120'}});fireEvent.click(screen.getByRole('button',{name:'Validate & stage'}));
 await screen.findByRole('button',{name:'Apply staged configuration'});expect(save).toHaveBeenCalledWith('gateway',expect.objectContaining({revision:0,values:expect.objectContaining({'/request_timeout_secs':120}),environment:{LLMCONDUIT_REQUEST_TIMEOUT_SECS:null}}));expect(apply).not.toHaveBeenCalled();fireEvent.click(screen.getByRole('button',{name:'Apply staged configuration'}));await waitFor(()=>expect(apply).toHaveBeenCalledWith(1));
});
it('keeps a draft after a failed save and discards it explicitly',async()=>{const client=getConnection().client;vi.spyOn(client,'stageSettings').mockRejectedValue(new Error('Configuration changed; reload'));renderWithQuery(<ConfigurationView scope="gateway"/>);await screen.findByLabelText('Request timeout');fireEvent.change(screen.getByLabelText('Request timeout'),{target:{value:'123'}});fireEvent.click(screen.getByRole('button',{name:'Validate & stage'}));expect(await screen.findByRole('alert')).toHaveTextContent('Configuration changed');expect(screen.getByLabelText('Request timeout')).toHaveValue(123);fireEvent.click(screen.getByRole('button',{name:'Discard edits'}));expect(screen.getByLabelText('Request timeout')).toHaveValue(600)});
it('keeps credentials masked while typing a replacement',async()=>{renderWithQuery(<ConfigurationView scope="authentication"/>);await screen.findByLabelText('Dashboard session signing key');const input=screen.getByLabelText('Dashboard session signing key');expect(input).toHaveAttribute('type','password');fireEvent.change(input,{target:{value:'replacement'}});expect(input).toHaveAttribute('type','password');expect(input).not.toHaveValue(SECRET)});
it('validates settings and host contracts rather than treating a legacy deployment report as settings',()=>{expect(isHostSettings({deployments:[]})).toBe(false);expect(isSettingsView({revision:1})).toBe(false);expect(overridesFor('/control_plane/storage/url')).toEqual(['LLMCONDUIT_DATABASE_URL']);expect(changedSettings({a:1,b:false},{a:2,b:false})).toEqual(['a'])});

it('removes a row immediately into a local draft and restores it on discard',async()=>{
 const client=getConnection().client;const stage=vi.spyOn(client,'stageSettings');renderWithQuery(<ConfigurationView scope="providers"/>);
 const remove=await screen.findByRole('button',{name:'Remove'});expect(screen.queryByRole('button',{name:'Edit settings'})).toBeNull();fireEvent.click(remove);
 expect(screen.queryByRole('button',{name:'Remove'})).toBeNull();expect(stage).not.toHaveBeenCalled();fireEvent.click(screen.getByRole('button',{name:'Discard edits'}));expect(screen.getByRole('button',{name:'Remove'})).toBeEnabled();
});
it('begins a frozen draft on Add and discards unfinished entry editors',async()=>{
 const client=getConnection().client;const stage=vi.spyOn(client,'stageSettings');renderWithQuery(<ConfigurationView scope="providers"/>);
 fireEvent.click(await screen.findByRole('button',{name:'Add backend'}));expect(screen.getByRole('button',{name:'Reload'})).toBeDisabled();expect(screen.getByLabelText('Finish entry editing')).toBeInvalid();expect(stage).not.toHaveBeenCalled();
 fireEvent.click(screen.getByRole('button',{name:'Discard edits'}));expect(screen.queryByRole('button',{name:'Update draft'})).toBeNull();expect(screen.getByRole('button',{name:'Reload'})).toBeEnabled();expect(screen.queryByText(/Advanced JSON/)).toBeNull();
});

it('freezes the original revision when an entry opens even if the query cache refreshes',async()=>{
 const client=getConnection().client;const stage=vi.spyOn(client,'stageSettings');const {queryClient}=renderWithQuery(<ConfigurationView scope="providers"/>);
 fireEvent.click(await screen.findByRole('button',{name:'Add backend'}));const saved=queryClient.getQueryData(['settings','providers']) as Awaited<ReturnType<typeof client.settings>>;
 act(()=>queryClient.setQueryData(['settings','providers'],{...saved,revision:99}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'New backend'}});fireEvent.change(screen.getByLabelText('Base URL'),{target:{value:'http://host/v1'}});fireEvent.click(screen.getByRole('button',{name:'Update draft'}));fireEvent.click(screen.getByRole('button',{name:'Validate & stage'}));
 await waitFor(()=>expect(stage).toHaveBeenCalledWith('providers',expect.objectContaining({revision:saved.revision})));expect(saved.revision).not.toBe(99);
});
it('keeps row mutations unavailable to a non-administrator',async()=>{
 getConnection();act(()=>authStore.getState().setUser({id:'reader',username:'reader',is_admin:false}));renderWithQuery(<ConfigurationView scope="providers"/>);
 await screen.findByRole('button',{name:'Review'});expect(screen.queryByRole('button',{name:'Add backend'})).toBeNull();expect(screen.queryByRole('button',{name:'Remove'})).toBeNull();expect(screen.queryByRole('button',{name:'Validate & stage'})).toBeNull();
});
