import {afterEach,it,expect,vi} from 'vitest';
import {cleanup,fireEvent,render,screen,within} from '@testing-library/react';
import {MetricIntervalField} from './MetricIntervalField';
import {SettingsField} from './SettingsField';
import {hostPolicySchema,schemaFor} from './settingsSchema';
import {SECRET,type Json} from './settingsModel';
afterEach(()=>cleanup());
it('edits nested metrics with units while preserving extension fields',()=>{
 const change=vi.fn();render(<SettingsField schema={schemaFor('/control_plane/metrics')} value={{enabled:true,scrape_interval_secs:15,backends:{},extension:{keep:true}}} onChange={change}/>);
 expect(screen.queryByRole('textbox',{name:'Backend metrics JSON'})).toBeNull();fireEvent.change(screen.getByLabelText('Scrape interval'),{target:{value:'10'}});
 expect(change).toHaveBeenLastCalledWith({enabled:true,scrape_interval_secs:10,backends:{},extension:{keep:true}});expect(screen.getByLabelText('Scrape interval')).toHaveAttribute('min','1');
});
it('uses readable backend names and changes ordered references without changing IDs',()=>{
 const change=vi.fn();const schema=schemaFor('/control_plane/operational/model_profiles').item!.fields!.backends!;
 render(<SettingsField schema={schema} value={['first','second']} references={{backends:[{id:'first',name:'GPU backend'},{id:'second',name:'Spark backend'}]}} onChange={change}/>);
 expect(screen.getByLabelText('Backend order 1')).toHaveDisplayValue('GPU backend');fireEvent.click(screen.getByRole('button',{name:'Move Backend order 2 up'}));expect(change).toHaveBeenCalledWith(['second','first']);
});
it('copies a backend with a new identity and clears retained credentials',()=>{
 const change=vi.fn();render(<SettingsField schema={schemaFor('/control_plane/operational/backends')} value={[{id:'old-id',name:'Original',base_url:'http://host:8000/v1',enabled:true,api_key:SECRET,api_key_env:'PRIVATE_BACKEND_KEY',custom:42}]} onChange={change}/>);
 fireEvent.click(screen.getByRole('button',{name:'Copy'}));expect(screen.getByLabelText('API credential')).toHaveAttribute('type','password');expect(screen.getByLabelText('API credential')).toHaveValue('');fireEvent.click(screen.getByRole('button',{name:'Update draft'}));
 const entries=change.mock.lastCall![0] as Json[];const copy=entries[1] as Record<string,Json>;expect(copy.id).not.toBe('old-id');expect(copy.name).toBe('Original-copy');expect(copy.api_key).toBeNull();expect(copy.api_key_env).toBeNull();expect(copy.custom).toBe(42);
});
it('leaves configured secrets untouched until replaced or explicitly cleared',()=>{
 const change=vi.fn();render(<SettingsField schema={schemaFor('/upstream_api_key')} value={SECRET} onChange={change}/>);
 expect(change).not.toHaveBeenCalled();expect(screen.getByLabelText('API credential')).toHaveValue('');fireEvent.click(screen.getByRole('button',{name:'Clear credential'}));expect(change).toHaveBeenCalledWith('');
});
it('adds named templates without JSON and rejects duplicate names',()=>{
 const change=vi.fn();render(<SettingsField schema={schemaFor('/model_profile_templates')} value={{existing:{upstream_model:'old'}}} onChange={change}/>);
 fireEvent.click(screen.getByRole('button',{name:'Add profile template'}));fireEvent.change(screen.getByLabelText('Reusable profile templates entry name'),{target:{value:'existing'}});fireEvent.change(screen.getByLabelText('Served model'),{target:{value:'new'}});fireEvent.click(screen.getByRole('button',{name:'Update draft'}));expect(screen.getByRole('alert')).toHaveTextContent('already exists');expect(change).not.toHaveBeenCalled();
 fireEvent.change(screen.getByLabelText('Reusable profile templates entry name'),{target:{value:'new-template'}});fireEvent.click(screen.getByRole('button',{name:'Update draft'}));expect(change).toHaveBeenCalledWith({existing:{upstream_model:'old'},'new-template':{upstream_model:'new'}});
});
it('keeps physical GPU totals read-only and gives memory budgets numeric controls',()=>{
 const change=vi.fn();render(<SettingsField schema={hostPolicySchema} value={{memory_budget_mib:1234,gpu_memory:{'0':{total_mib:96000,budget_mib:90000,unified_memory:false}}}} onChange={change}/>);
 const gpuTable=screen.getByText('GPU memory budgets').closest('[data-testid="panel-header"]')!.parentElement!;fireEvent.click(within(gpuTable).getByRole('button',{name:'Edit'}));expect(screen.getByLabelText('Detected total')).toBeDisabled();expect(screen.getByLabelText('Reservation budget')).not.toBeDisabled();expect(screen.getByLabelText('Host RAM budget')).toHaveAttribute('type','number');
});
it('distinguishes automatic image capability from explicit false',()=>{
 const change=vi.fn();render(<SettingsField schema={schemaFor('/model_profiles').item!.fields!.native_vision!} value={undefined} onChange={change}/>);
 expect(screen.getByLabelText('Native image input')).toHaveDisplayValue('Automatic / inherited');fireEvent.change(screen.getByLabelText('Native image input'),{target:{value:'false'}});expect(change).toHaveBeenCalledWith(false);
});
it('edits backend-specific nested options with typed controls',()=>{
 const change=vi.fn();render(<SettingsField schema={schemaFor('/upstream_chat_kwargs')} value={{temperature:.3,extension:{keep:false}}} onChange={change}/>);
 fireEvent.click(screen.getByRole('button',{name:'Edit'}));expect(screen.getByLabelText('extension type')).toHaveValue('map');
 expect(screen.queryByText(/Advanced JSON/)).toBeNull();expect(screen.getByText('keep',{exact:true})).toBeVisible();fireEvent.click(screen.getAllByRole('button',{name:'Edit'}).at(-1)!);fireEvent.click(screen.getByLabelText('keep value'));fireEvent.click(screen.getAllByRole('button',{name:'Update draft'}).at(-1)!);fireEvent.click(screen.getByRole('button',{name:'Update draft'}));expect(change).toHaveBeenCalledWith({temperature:.3,extension:{keep:true}});
});
it('supports regex header extraction and recursive client match conditions',()=>{
 const schema=schemaFor('/control_plane/sessions').fields!.harnesses!.item!.fields!;const change=vi.fn();
 const {rerender}=render(<SettingsField schema={schema.session_id!} value={{header:'x-session'}} onChange={change}/>);
 expect(screen.getByLabelText('Header name')).toHaveValue('x-session');fireEvent.change(screen.getByLabelText('Regular expression'),{target:{value:'^(.+)$'}});expect(change).toHaveBeenCalledWith({header:{name:'x-session',regex:'^(.+)$'}});
 rerender(<SettingsField schema={schema.match!} value={{always:true}} onChange={change}/>);fireEvent.change(screen.getByLabelText('Match client source'),{target:{value:'all'}});expect(change).toHaveBeenCalledWith({all:[]});
});
it('writes role rules in their backend wire layout',()=>{
 const change=vi.fn();const schema=schemaFor('/model_profiles').item!.fields!.roles!;
 render(<SettingsField schema={schema} value={{user:{action:'accept'}}} onChange={change}/>);
 fireEvent.click(screen.getByRole('button',{name:'Edit'}));fireEvent.change(screen.getByLabelText('Action'),{target:{value:'drop'}});fireEvent.click(screen.getByRole('button',{name:'Update draft'}));expect(change).toHaveBeenCalledWith({user:{action:'drop'}});
});

it('gives sampling intervals numeric bounds, units and an explicit collection toggle',()=>{
 const change=vi.fn();render(<MetricIntervalField metric="gpu_util" value="100ms" onChange={change}/>);expect(screen.getByLabelText('Gpu util sampling interval')).toHaveValue(100);expect(screen.getByLabelText('Gpu util sampling interval')).toHaveAttribute('min','100');fireEvent.change(screen.getByLabelText('Gpu util interval unit'),{target:{value:'s'}});expect(screen.getByLabelText('Gpu util sampling interval')).toHaveValue(.1);fireEvent.change(screen.getByLabelText('Gpu util sampling interval'),{target:{value:'.5'}});expect(change).toHaveBeenCalledWith('0.5s');fireEvent.click(screen.getByLabelText('Collect Gpu util'));expect(change).toHaveBeenCalledWith('off');
});
it('uses the one-second lower bound for container sampling',()=>{render(<MetricIntervalField metric="container_cpu" value="5s" onChange={()=>{}}/>);expect(screen.getByLabelText('Container cpu sampling interval')).toHaveAttribute('min','1')});

it('explains a model profile on hover and keyboard focus without changing configuration',()=>{
 const change=vi.fn();render(<SettingsField schema={schemaFor('/control_plane/operational/model_profiles')} value={[]} onChange={change}/>);
 const help=screen.getByRole('button',{name:'Help: Model profiles'});fireEvent.mouseEnter(help.parentElement!);expect(screen.getByRole('tooltip')).toHaveTextContent('A model profile combines');fireEvent.mouseLeave(help.parentElement!);expect(screen.queryByRole('tooltip')).toBeNull();fireEvent.focus(help);expect(screen.getByRole('tooltip')).toBeVisible();fireEvent.keyDown(help,{key:'Escape'});expect(screen.queryByRole('tooltip')).toBeNull();expect(change).not.toHaveBeenCalled();
});
