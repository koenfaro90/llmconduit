import {beforeEach,afterEach,describe,it,expect,vi} from 'vitest';
import {act,cleanup,fireEvent,screen,waitFor} from '@testing-library/react';
import {RequestBodyTabs} from './RequestBodyTabs';
import {decodeExact,exchanges,partFor,type WireManifest,type WirePart} from './wireModel';
import {renderWithQuery,resetWorld} from '../testHarness';
import {getConnection} from '../../api/connection';
import {MOCK_HISTORY_REQUESTS} from '../../api/mock';
vi.mock('../ui/JsonEditor',()=>({JsonEditor:({value,label}:{value:string;label:string})=><textarea readOnly aria-label={label} value={value}/>}));
const copy=vi.fn().mockResolvedValue(undefined);
const request={...MOCK_HISTORY_REQUESTS[0]!,status:'completed'};
const raw='\ufeff { "api_key" : "unchanged", "tool": "\\u0061" }\r\n'+ 'x'.repeat(300_000)+'\nLAST-BYTE';
const part=(file:string):WirePart=>({file:file+'.body',chunks:[],headers:[],method:'POST',target:'http://upstream/v1/chat/completions',status:200,bytes:new TextEncoder().encode(raw).length,sha256:null,complete:true,error:null});
const manifest:WireManifest={schema:'llmconduit-exact-http/v1',api_call_id:request.id,parts:Object.fromEntries(['client_in','upstream_000001_out','upstream_000001_in','client_out'].map(k=>[k,part(k)]))};
beforeEach(()=>{resetWorld({mock:true});Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:copy}});copy.mockClear();});afterEach(()=>{cleanup();vi.restoreAllMocks();});
describe('complete HTTP body tabs',()=>{
 it('copies every original character including BOM, escapes, CRLF and content beyond the old limit',async()=>{
  const {client}=getConnection();vi.spyOn(client,'wireManifest').mockResolvedValue(manifest);const read=vi.spyOn(client,'historyBodyBytes').mockResolvedValue(new TextEncoder().encode(raw));
  renderWithQuery(<RequestBodyTabs request={request}/>);
  for(const [label,key] of [['Inbound request','client_in'],['Upstream request','upstream_000001_out'],['Upstream response','upstream_000001_in'],['Inbound response','client_out']]){
   fireEvent.click(screen.getByRole('tab',{name:label}));await waitFor(()=>expect(screen.getByRole('button',{name:'Copy body'})).toBeEnabled());fireEvent.click(screen.getByRole('button',{name:'Copy body'}));await waitFor(()=>expect(copy).toHaveBeenLastCalledWith(raw));expect(read).toHaveBeenCalledWith(request.id,key);
  }
  expect(screen.queryByRole('button',{name:'Format JSON'})).not.toBeInTheDocument();expect(screen.getAllByRole('tab')).toHaveLength(5);
 });
 it('keeps every upstream call selectable instead of overwriting failed attempts',async()=>{
  const {client}=getConnection();vi.spyOn(client,'wireManifest').mockResolvedValue({...manifest,parts:{...manifest.parts,upstream_000002_out:part('upstream_000002_out'),upstream_000002_in:part('upstream_000002_in')}});const read=vi.spyOn(client,'historyBodyBytes').mockResolvedValue(new TextEncoder().encode('data: [DONE]\r\n\r\n'));
  renderWithQuery(<RequestBodyTabs request={request}/>);fireEvent.click(screen.getByRole('tab',{name:'Upstream response'}));await screen.findByLabelText('Upstream call');fireEvent.change(screen.getByLabelText('Upstream call'),{target:{value:'upstream_000001'}});await waitFor(()=>expect(read).toHaveBeenCalledWith(request.id,'upstream_000001_in'));
 });
 it('labels old altered captures honestly and still exposes stored response bytes',async()=>{
  const {client}=getConnection();vi.spyOn(client,'wireManifest').mockResolvedValue(null);vi.spyOn(client,'historyBodyBytes').mockResolvedValue(new TextEncoder().encode('{}'));vi.spyOn(client,'storedRequestDetail').mockResolvedValue({request,events:[{hop:'client_out',kind:'response',ts_ms:1,payload:JSON.stringify({content:'data: {"tool_calls":[]}\r\n\r\n'})}],events_truncated:false});
  renderWithQuery(<RequestBodyTabs request={request}/>);await screen.findByTestId('legacy-wire-notice');fireEvent.click(screen.getByRole('tab',{name:'Inbound response'}));await waitFor(()=>expect(screen.getByRole('button',{name:'Copy legacy capture'})).toBeEnabled());fireEvent.click(screen.getByRole('button',{name:'Copy legacy capture'}));await waitFor(()=>expect(copy).toHaveBeenCalledWith('data: {"tool_calls":[]}\r\n\r\n'));
 });
 it('reloads the body when a recording completes, so a cached prefix cannot become the final body',async()=>{
  const {client,queryClient}=getConnection();const pending={...manifest,parts:{...manifest.parts,client_in:{...manifest.parts.client_in!,complete:false,bytes:0,sha256:null}}};
  vi.spyOn(client,'wireManifest').mockResolvedValue(pending);vi.spyOn(client,'historyBodyBytes').mockResolvedValueOnce(new TextEncoder().encode('prefix')).mockResolvedValue(new TextEncoder().encode('prefix and final bytes'));
  renderWithQuery(<RequestBodyTabs request={request}/>);await waitFor(()=>expect(screen.getByLabelText('Inbound request')).toHaveValue('prefix'));
  act(()=>{queryClient.setQueryData(['history','requests',request.id,'wire'],{...manifest,parts:{...manifest.parts,client_in:{...manifest.parts.client_in!,sha256:'final-hash'}}});});
  await waitFor(()=>expect(screen.getByLabelText('Inbound request')).toHaveValue('prefix and final bytes'));fireEvent.click(screen.getByRole('button',{name:'Copy body'}));await waitFor(()=>expect(copy).toHaveBeenCalledWith('prefix and final bytes'));
 });
 it('does not substitute replacement characters for binary/compressed wire bytes',()=>{
  expect(decodeExact(new Uint8Array([0xff,0x80,0]))).toBeNull();expect(decodeExact(new TextEncoder().encode('\ufeff\r\n\u0000'))).toBe('\ufeff\r\n\u0000');
  expect(exchanges(manifest)).toEqual(['upstream_000001']);expect(partFor('upstream-response','upstream_000001')).toBe('upstream_000001_in');
 });
});
