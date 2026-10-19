import {useState} from 'react';
import {Button} from '../ui/Button';
import {PanelHeader} from '../ui/Panel';
import {DeltasPanel} from './DeltasPanel';
import {outputSegments,type AssembledOutput} from './assembledOutput';
export function AssembledOutputPanel({output,fallback}:{output:AssembledOutput|null;fallback?:React.ReactNode}){
 const [copied,setCopied]=useState('');
 if(!output?.items.length)return <div className="min-h-0 overflow-auto">{(!output||output.partial)&&fallback?fallback:<p className="text-xs text-text-muted">No readable model output was recorded. The original response remains available in the response tabs.</p>}</div>;
 const answer=output.items.filter(i=>i.kind!=='reasoning');const thoughts=output.items.filter(i=>i.kind==='reasoning');
 async function copy(){try{await navigator.clipboard.writeText(outputSegments(output!.items).map(s=>s.kind==='reasoning'?'Reasoning\n'+s.text:s.text).join('\n\n'));setCopied('Copied');}catch{setCopied('Clipboard unavailable');}}
 return <div className="flex min-h-0 flex-1 flex-col gap-2" data-testid="assembled-output">
  <PanelHeader label="Assembled output" help="Readable text, reasoning and tool arguments assembled from the stored response. This is a derived reference view. Response tabs and their Copy/Download controls retain the original wire bytes." actions={<Button onClick={()=>void copy()}>Copy assembled output</Button>}><h3 className="text-sm font-semibold">Assembled output</h3></PanelHeader>
  {copied&&<p role="status" className="text-xs">{copied}</p>}
  {output.partial&&<p role="status" className="text-xs text-status-cooling">Partial response: some stream data could not yet be assembled. The raw response is preserved.</p>}
  <div className="min-h-0 flex-1 overflow-auto rounded border border-line bg-bg/40">
   {answer.length>0?<>{answer.map(item=><DeltasPanel key={item.id+item.kind} segments={outputSegments([item])}/>)}</>:<p className="p-3 text-xs text-text-muted">No answer text or tool calls in this response.</p>}
   {thoughts.length>0&&<details className="border-t border-line p-2"><summary className="cursor-pointer text-xs text-text-muted">Reasoning</summary>{thoughts.map(item=><DeltasPanel key={item.id} segments={outputSegments([item])}/>) }</details>}
  </div>
 </div>;
}
