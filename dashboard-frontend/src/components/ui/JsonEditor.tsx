import {useEffect,useRef,useState} from 'react';
import type {editor} from 'monaco-editor';
import {Button} from './Button';
/** Lazy, locally bundled Monaco. No source is uploaded to an editor service. */
export function JsonEditor({value,onChange,readOnly=false,label='Recipe specification'}:{value:string;onChange:(value:string)=>void;readOnly?:boolean;label?:string}) {
  const container=useRef<HTMLDivElement>(null);const instance=useRef<editor.IStandaloneCodeEditor>();const latest=useRef({value,onChange,readOnly});latest.current={value,onChange,readOnly};const [error,setError]=useState('');
  useEffect(()=>{let disposed=false;let model:editor.ITextModel|undefined;let listener:{dispose:()=>void}|undefined;
    void import('./monacoSetup').then(monaco=>{if(disposed||!container.current)return;
      model=monaco.editor.createModel(latest.current.value,'json');
      instance.current=monaco.editor.create(container.current,{model,theme:'vs-dark',readOnly:latest.current.readOnly,automaticLayout:true,minimap:{enabled:false},scrollBeyondLastLine:false,wordWrap:'on',fontSize:12,tabSize:2,ariaLabel:label,accessibilitySupport:'auto',fixedOverflowWidgets:true});
      listener=instance.current!.onDidChangeModelContent(()=>latest.current.onChange(instance.current?.getValue()??''));
    }).catch(()=>{if(!disposed)setError('The JSON editor could not load. Reload to retry.');});
    return()=>{disposed=true;listener?.dispose();instance.current?.dispose();instance.current=undefined;model?.dispose();};
  },[label]);
  useEffect(()=>{if(instance.current&&instance.current.getValue()!==value)instance.current.setValue(value);},[value]);
  useEffect(()=>{instance.current?.updateOptions({readOnly});},[readOnly]);
  return <div className="min-w-0 space-y-2"><div className="flex items-center justify-between text-xs"><label>{label}</label><Button type="button" variant="ghost" disabled={readOnly} onClick={()=>void instance.current?.getAction('editor.action.formatDocument')?.run()}>Format JSON</Button></div>{error&&<p role="alert">{error}</p>}<div ref={container} aria-label={label} className="h-80 min-w-0 overflow-hidden rounded border border-line" /></div>;
}
