import type {DebugSegment} from '../../api/types';
import {bodyPresentation} from './wireModel';

type Obj=Record<string,unknown>;
export interface OutputItem {id:string;kind:'output'|'reasoning'|'tool';text:string;name?:string}
export interface AssembledOutput {items:OutputItem[];stopReason:string|null;terminal:boolean;partial:boolean;unparsedEvents:number}
const object=(v:unknown):Obj=>v&&typeof v==='object'&&!Array.isArray(v)?v as Obj:{};
const string=(v:unknown):string=>typeof v==='string'?v:'';
const list=(v:unknown):unknown[]=>Array.isArray(v)?v:[];
function text(v:unknown):string {
 if(typeof v==='string')return v;
 return list(v).map(part=>{const p=object(part);return string(p.text)||string(p.thinking)||string(p.refusal);}).join('');
}
function reasoning(v:Obj):string {
 for(const key of ['reasoning_content','reasoning','reasoning_text','reasoning_delta','reasoning_summary','thinking','thinking_content']){
  const value=v[key];if(typeof value==='string'&&value)return value;
  const nested=object(value);for(const k of ['text','delta','content','summary','thinking'])if(typeof nested[k]==='string'&&nested[k])return nested[k] as string;
 }
 return '';
}

/** Derived display only. Original response bytes and exports are never rewritten. */
export function assembleOutput(body:string):AssembledOutput {
 body=body.replace(/^\uFEFF/,'');
 let items=new Map<string,OutputItem>();let stopReason:string|null=null,partial=false,unparsedEvents=0,terminal=false;
 const indexes=new Map<number,string>();
 const put=(id:string,kind:OutputItem['kind'],value:string,append:boolean,name?:string)=>{
  const key=id+':'+kind;const prior=items.get(key);items.set(key,{id,kind,text:append?(prior?.text??'')+value:value,name:name||prior?.name});
 };
 const snapshot=(value:unknown,index:number)=>{
  const item=object(value);const id=string(item.id)||`item-${index}`;indexes.set(index,id);
  if(item.type==='reasoning')put(id,'reasoning',text(item.content)||text(item.summary),false);
  else if(item.type==='function_call'||item.type==='tool_use')put(id,'tool',typeof item.arguments==='string'?item.arguments:JSON.stringify(item.input??item.arguments??{}),false,string(item.name));
  else if(item.type==='thinking')put(id,'reasoning',string(item.thinking),false);
  else if(item.type==='text')put(id,'output',string(item.text),false);
  else if(item.type==='message')put(id,'output',text(item.content),false);
 };
 const consume=(value:unknown,eventName='')=>{
  const event=object(value);const type=string(event.type)||eventName;
  const response=object(event.response);
  // Final Responses snapshots replace streamed fragments, so the same text is
  // never repeated when both deltas and a completed response are retained.
  if(Array.isArray(response.output)||Array.isArray(event.output)){
   const source=Array.isArray(response.output)?response:event;items=new Map();indexes.clear();list(source.output).forEach(snapshot);
   stopReason=string(object(source.incomplete_details).reason)||string(source.status)||type||stopReason;
  }
  if(type==='response.output_item.added'||type==='response.output_item.done')snapshot(event.item,Number(event.output_index??0));
  const id=string(event.item_id)||indexes.get(Number(event.output_index??0))||`item-${Number(event.output_index??0)}`;
  if(type==='response.output_text.delta'||type==='response.refusal.delta')put(id,'output',string(event.delta),true);
  if(type==='response.reasoning_text.delta'||type==='response.reasoning_summary_text.delta')put(id,'reasoning',string(event.delta),true);
  if(type==='response.function_call_arguments.delta')put(id,'tool',string(event.delta),true);
  if(type==='response.output_text.done')put(id,'output',string(event.text),false);
  if(type==='response.function_call_arguments.done')put(id,'tool',string(event.arguments),false);
  if(type==='response.completed'||type==='response.incomplete'||type==='response.failed'){terminal=true;stopReason=string(object(response.incomplete_details).reason)||type;}
  if(type==='message_stop')terminal=true;
  // Chat Completions: choices and tool indexes are independent streams.
  for(const raw of list(event.choices)){
   const choice=object(raw);const message=object(choice.message??choice.delta);const append=choice.message==null;const key=`choice-${Number(choice.index??0)}`;
   const content=text(message.content)||string(message.refusal);if(content)put(key,'output',content,append);
   const thought=reasoning(message);if(thought)put(key,'reasoning',thought,append);
   for(const [i,rawCall] of list(message.tool_calls).entries()){
    const call=object(rawCall),fn=object(call.function);const toolId=key+':tool-'+Number(call.index??i);
    put(toolId,'tool',typeof fn.arguments==='string'?fn.arguments:'',append,string(fn.name));
   }
   if(message.function_call){const fn=object(message.function_call);put(key+':function','tool',string(fn.arguments),append,string(fn.name));}
   if(choice.finish_reason!=null)stopReason=string(choice.finish_reason)||stopReason;
  }
  // Anthropic Messages, including text, thinking and fragmented tool JSON.
  const blockId=`block-${Number(event.index??0)}`;
  if(type==='content_block_start'){
   const block=object(event.content_block);
   if(block.type==='tool_use')put(blockId,'tool',Object.keys(object(block.input)).length?JSON.stringify(block.input):'',false,string(block.name));
   else put(blockId,block.type==='thinking'?'reasoning':'output',string(block.thinking)||string(block.text),false);
  }
  if(type==='content_block_delta'){
   const delta=object(event.delta);if(delta.type==='thinking_delta')put(blockId,'reasoning',string(delta.thinking),true);
   if(delta.type==='text_delta')put(blockId,'output',string(delta.text),true);
   if(delta.type==='input_json_delta')put(blockId,'tool',string(delta.partial_json),true);
  }
  if(type==='message_delta')stopReason=string(object(event.delta).stop_reason)||stopReason;
  if(Array.isArray(event.content)&&event.role==='assistant'){
   items=new Map();list(event.content).forEach(snapshot);stopReason=string(event.stop_reason)||stopReason;
  }
 };
 try {const value=JSON.parse(body);consume(value);const v=object(value);terminal=terminal||Array.isArray(v.choices)&&v.choices.length>0&&v.choices.every(c=>object(c).finish_reason!=null)||Array.isArray(v.output)&&['completed','incomplete','failed'].includes(string(v.status))||v.role==='assistant'&&v.stop_reason!=null;}
 catch {
  let sawEvent=false,done=false;
  for(const block of body.replace(/\r\n/g,'\n').split(/\n\n/)){
   const lines=block.split('\n').filter(line=>line.startsWith('data:'));
   if(!lines.length)continue;sawEvent=true;
   const data=lines.map(line=>line.slice(5).replace(/^ /,'')).join('\n');if(data==='[DONE]'){done=true;terminal=true;continue;}
   const eventName=block.split('\n').find(line=>line.startsWith('event:'))?.slice(6).trim()??'';
   try{consume(JSON.parse(data),eventName);}catch{unparsedEvents++;}
  }
  partial=unparsedEvents>0||!sawEvent||(!stopReason&&!done&&!terminal);
 }
 return {items:[...items.values()].filter(item=>item.text||item.kind==='tool'),stopReason,terminal,partial,unparsedEvents};
}

export function outputSegments(items:OutputItem[]):DebugSegment[] {
 return items.map(item=>{
  let value=item.text;
  if(item.kind==='tool'){
   let args:string;try{JSON.parse(value);args=value;}catch{args=JSON.stringify(value);}
   value=`tool arguments ${item.id}:\n`+bodyPresentation(`{"name":${JSON.stringify(item.name??'function')},"arguments":${args}}`).value;
  }
  return {timestamp_ms:0,kind:item.kind==='output'?'output':item.kind==='reasoning'?'reasoning':'tool',text:value};
 });
}
