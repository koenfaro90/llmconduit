import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import {cleanup,fireEvent,screen,waitFor} from '@testing-library/react';
import {renderWithQuery,resetWorld} from '../../components/testHarness';
import {getConnection} from '../../api/connection';
import {TrafficView} from './TrafficView';
import {blankConfig,blankRuntime,backendRows} from './trafficModel';
beforeEach(()=>resetWorld({mock:true}));afterEach(()=>{cleanup();vi.restoreAllMocks();});
it('keeps controls opt-in and saves an explicit revision through the shared API',async()=>{
    const save=vi.spyOn(getConnection().client,'saveTraffic');
    renderWithQuery(<TrafficView/>);await screen.findByTestId('traffic-view');
    expect(screen.getByText(/Enable observation or enforcement/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab',{name:'Policies'}));
    expect(screen.getByLabelText('Mode')).toHaveValue('off');
    expect(save).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText('Mode'),{target:{value:'observe'}});
    const button=screen.getByRole('button',{name:'Save policy'});await waitFor(()=>expect(button).toBeEnabled());fireEvent.click(button);
    await waitFor(()=>expect(save).toHaveBeenCalledWith(0,expect.objectContaining({mode:'observe',group_members:{},queue:expect.objectContaining({enabled:false})})));
    await screen.findByText(/Policy saved/);
});
it('shows the strictest shared physical backend cap and all linked attempts',()=>{
    const config=blankConfig();config.backends={'http://a/v1':{max_concurrent:4,paused:false,resource:'gpu',deployment_id:null},'http://b/v1':{max_concurrent:2,paused:true,resource:'gpu',deployment_id:null}};
    const runtime=blankRuntime();runtime.active=[{id:'one',backend:'http://a/v1',model:'m',key:'k',user:'u',groups:[],budget:'m',priority:1,created_at_ms:0,state:'streaming',reason:null,bytes:1,affinity:'off'}];
    expect(backendRows(runtime,config,[])).toEqual(expect.arrayContaining([expect.objectContaining({url:'http://b/v1',limit:2,paused:true,active:1}),expect.objectContaining({url:'http://a/v1',limit:2,paused:true,active:1})]));
});
