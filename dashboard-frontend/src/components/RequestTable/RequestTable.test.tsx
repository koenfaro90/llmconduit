import {afterEach,beforeEach,expect,it} from 'vitest';
import {cleanup,fireEvent,screen,within} from '@testing-library/react';
import {renderWithQuery,resetWorld} from '../testHarness';
import {RequestTable} from './RequestTable';
import {MOCK_HISTORY_REQUESTS} from '../../api/mock';
import {inboundSetting} from './requestsModel';

beforeEach(()=>{resetWorld({mock:true});});afterEach(cleanup);
it('shows inbound settings, keeps false/absence distinct, and filters/sorts the shared table',()=>{
 const first={...MOCK_HISTORY_REQUESTS[0]!,id:'first',inbound_settings:{reasoning_effort:'high',thinking:false,clear_thinking:true,max_tokens:8192}};
 const second={...first,id:'second',inbound_settings:{reasoning_effort:'low',thinking:{type:'enabled'},clear_thinking:false,max_tokens:1024}};
 expect(inboundSetting(first,'thinking')).toBe('false');expect(inboundSetting({...first,inbound_settings:null},'thinking')).toBeNull();
 renderWithQuery(<RequestTable rows={[first,second]} columns={['reasoningEffort','thinking','clearThinking','maxTokens']}/>);
 expect(screen.getAllByText('false',{selector:'td'})).toHaveLength(2);expect(screen.getByText('{"type":"enabled"}')).toBeInTheDocument();
 fireEvent.click(screen.getByRole('button',{name:'Sort max_tokens'}));
 const rows=screen.getAllByTestId('request-row');expect(within(rows[0]!).getByText('1024')).toBeInTheDocument();
 fireEvent.change(screen.getByRole('textbox',{name:'Filter Reasoning effort'}),{target:{value:'high'}});
 expect(screen.getAllByTestId('request-row')).toHaveLength(1);expect(screen.getByText('8192')).toBeInTheDocument();
});
