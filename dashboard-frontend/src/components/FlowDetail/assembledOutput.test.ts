import {expect,it} from 'vitest';
import {assembleOutput,outputSegments} from './assembledOutput';
const sse=(...events:unknown[])=>events.map(e=>'data: '+JSON.stringify(e)+'\r\n\r\n').join('')+'data: [DONE]\r\n\r\n';
it('assembles chat reasoning, Unicode text and interleaved tool calls without merging choices',()=>{
 const output=assembleOutput(sse(
  {choices:[{index:0,delta:{role:'assistant',reasoning:'Think '}}]},
  {choices:[{index:0,delta:{reasoning:'first',content:'Hé'}}]},
  {choices:[{index:0,delta:{content:'llo',tool_calls:[{index:0,function:{name:'read',arguments:'{"n":900719925474099312345'}}]}}]},
  {choices:[{index:0,delta:{tool_calls:[{index:0,function:{arguments:'}'}}]},finish_reason:'tool_calls'},{index:1,delta:{content:'Other'}}]}
 ));
 expect(output.items.filter(i=>i.kind==='output').map(i=>i.text)).toEqual(['Héllo','Other']);
 expect(output.items.find(i=>i.kind==='reasoning')?.text).toBe('Think first');
 expect(output.items.find(i=>i.kind==='tool')).toMatchObject({name:'read',text:'{"n":900719925474099312345}'});
 expect(outputSegments(output.items).find(i=>i.kind==='tool')?.text).toContain('900719925474099312345');
 expect(output.stopReason).toBe('tool_calls');expect(output.partial).toBe(false);
});
it('uses authoritative Responses snapshots rather than duplicating streamed text or reasoning',()=>{
 const final=[{id:'r1',type:'reasoning',summary:[{type:'summary_text',text:'Think'}],content:[{type:'reasoning_text',text:'Think'}]}, {id:'m1',type:'message',content:[{type:'output_text',text:'Hello'}]}, {id:'f1',type:'function_call',name:'search',arguments:'{"q":"test"}'}];
 const result=assembleOutput(sse({type:'response.output_text.delta',item_id:'m1',delta:'Hel'}, {type:'response.output_text.delta',item_id:'m1',delta:'lo'}, {type:'response.completed',response:{status:'completed',output:final}}));
 expect(result.items.map(i=>i.text)).toEqual(['Think','Hello','{"q":"test"}']);expect(result.stopReason).toBe('response.completed');
});
it('assembles Anthropic text, thinking and tool JSON, and reads buffered responses',()=>{
 const result=assembleOutput(sse({type:'content_block_start',index:0,content_block:{type:'text',text:''}}, {type:'content_block_delta',index:0,delta:{type:'text_delta',text:'Answer'}}, {type:'content_block_start',index:1,content_block:{type:'tool_use',name:'lookup',input:{}}}, {type:'content_block_delta',index:1,delta:{type:'input_json_delta',partial_json:'{"id":1}'}}, {type:'message_delta',delta:{stop_reason:'tool_use'}}));
 expect(result.items.map(i=>i.text)).toEqual(['Answer','{"id":1}']);expect(result.stopReason).toBe('tool_use');
 expect(assembleOutput(JSON.stringify({choices:[{index:0,message:{content:'Buffered',reasoning_content:'Why'},finish_reason:'stop'}]})).items.map(i=>i.text)).toEqual(['Buffered','Why']);
});
it('labels unfinished/malformed streams while preserving earlier output and never imposing a cap',()=>{
 const huge='x'.repeat(300_000);const result=assembleOutput(sse({choices:[{delta:{content:huge}}]})+'data: {"choices":\n');
 expect(result.items[0]?.text).toHaveLength(300_000);expect(result.partial).toBe(true);expect(result.unparsedEvents).toBe(1);
 expect(assembleOutput('data: {"choices":[{"delta":{"content":"partial"}}]}\n\n').partial).toBe(true);
});

it('reads the SSE event field when the payload has no type property',()=>{
 const result=assembleOutput('event: response.output_text.delta\ndata: '+JSON.stringify({item_id:'m',delta:'Hello'})+'\n\nevent: response.completed\ndata: '+JSON.stringify({response:{status:'completed',output:[{id:'m',type:'message',content:[{type:'output_text',text:'Hello'}]}]}})+'\n\n');
 expect(result.items.map(i=>i.text)).toEqual(['Hello']);expect(result.stopReason).toBe('response.completed');expect(result.partial).toBe(false);
});

it('distinguishes terminal protocol markers from a single completed choice',()=>{
 const choice='data: '+JSON.stringify({choices:[{index:0,delta:{content:'Answer'},finish_reason:'stop'}]})+'\n\n';
 expect(assembleOutput(choice).terminal).toBe(false);expect(assembleOutput(choice+'data: [DONE]\n\n').terminal).toBe(true);
 expect(assembleOutput('event: message_stop\ndata: {"type":"message_stop"}\n\n').terminal).toBe(true);
});
