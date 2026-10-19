export interface WireHeader {name:string;value_base64:string}
export interface WireChunk {sha256:string;bytes:number}
export interface WirePart {file:string;chunks:WireChunk[];headers:WireHeader[];method:string|null;target:string|null;status:number|null;bytes:number;sha256:string|null;complete:boolean;error:string|null}
export interface WireManifest {schema:string;api_call_id:string;parts:Record<string,WirePart>}
export interface StoredRequestDetail {request:unknown;events:Array<{hop:string;kind:string;ts_ms:number;payload:string|null}>;events_truncated:boolean}
export function decodeExact(bytes:Uint8Array):string|null {try{return new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(bytes);}catch{return null;}}
export function headerValue(value:string):string {try{return decodeExact(Uint8Array.from(atob(value),c=>c.charCodeAt(0)))??`base64:${value}`;}catch{return '[invalid stored header]';}}
export function legacyResponse(detail:StoredRequestDetail|undefined,hop:string):Uint8Array|undefined {
 const event=detail?.events.find(e=>e.hop===hop);if(!event?.payload)return undefined;
 try{const envelope=JSON.parse(event.payload) as {content?:unknown};return typeof envelope.content==='string'?new TextEncoder().encode(envelope.content):undefined;}catch{return undefined;}
}
export function partFor(tab:string,exchange:string):string {
 if(tab==='inbound')return 'client_in';if(tab==='response')return 'client_out';return exchange+(tab==='upstream'?'_out':'_in');
}
export function exchanges(manifest:WireManifest|null|undefined):string[] {return [...new Set(Object.keys(manifest?.parts??{}).filter(k=>k.startsWith('upstream_')).map(k=>k.replace(/_(?:in|out)$/,'')))].sort();}

export function legacyHeaders(detail:StoredRequestDetail|undefined):Array<{id:string;part:string;name:string;value:string}>{
 const raw=detail?.events.find(e=>e.hop==='client_in')?.payload;if(!raw)return [];
 try{const headers=(JSON.parse(raw) as {headers?:Record<string,unknown>}).headers??{};return Object.entries(headers).filter((row):row is [string,string]=>typeof row[1]==='string').map(([name,value])=>({id:name,part:'client_in (legacy)',name,value}));}catch{return [];}
}

/** Display whitespace only: retain large numbers, duplicate keys and escapes. */
export function bodyPresentation(text:string):{value:string;language:'json'|'plaintext'} {
 try { JSON.parse(text); } catch { return {value:text,language:'plaintext'}; }
 const tokens=text.match(/"(?:\\.|[^"\\])*"|[{}[\],:]|[^\s{}[\],:]+/g)??[];
 let depth=0,value='';
 for(let i=0;i<tokens.length;i++) {
  const token=tokens[i]!,previous=tokens[i-1],next=tokens[i+1];
  if(token==='{'||token==='[') {value+=token;depth++;if(next!=='}'&&next!==']')value+='\n'+'  '.repeat(depth);}
  else if(token==='}'||token===']') {depth--;if(previous!=='{'&&previous!=='[')value+='\n'+'  '.repeat(depth);value+=token;}
  else if(token===',')value+=',\n'+'  '.repeat(depth);
  else if(token===':')value+=': ';
  else value+=token;
 }
 return {value,language:'json'};
}
