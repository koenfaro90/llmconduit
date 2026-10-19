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

it('shows, sorts and filters completed TTFT/decode values using the displayed measurements',()=>{
 const a={...MOCK_HISTORY_REQUESTS[0]!,id:'timing-a',display_number:1,status:'completed',created_at_ms:1000,completed_at_ms:4000,first_token_at_ms:1200,
   timings_json:JSON.stringify({first_upstream_byte_ms:1100,first_generation_delta_ms:1200,generation_duration_us:2_000_000,generation_tokens:400,generation_timing_complete:true})};
 const b={...a,id:'timing-b',display_number:2,first_token_at_ms:1500,timings_json:JSON.stringify({first_upstream_byte_ms:1400,first_generation_delta_ms:1500,generation_duration_us:2_000_000,generation_tokens:200,generation_timing_complete:true})};
 renderWithQuery(<RequestTable rows={[a,b]} columns={['number','ttft','decodeSpeed']}/>);
 expect(screen.getAllByTestId('request-decode-speed')[0]).toHaveAttribute('data-quality','derived');
 fireEvent.click(screen.getByRole('button',{name:'Sort Decode speed'}));
 expect(within(screen.getAllByTestId('request-row')[0]!).getByText('R-2')).toBeInTheDocument();
 fireEvent.change(screen.getByRole('textbox',{name:'Filter Decode speed'}),{target:{value:'200'}});
 expect(screen.getAllByTestId('request-row')).toHaveLength(1);
 expect(screen.getByText('R-1')).toBeInTheDocument();
});
