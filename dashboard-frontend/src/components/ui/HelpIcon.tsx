import {useId,useRef,useState} from 'react';
import {createPortal} from 'react-dom';

/** Hover/focus help, with a tap toggle for touch screens. Never submits a form. */
export function HelpIcon({label,children}:{label:string;children:string}){
 const id=useId();const button=useRef<HTMLButtonElement>(null);const [open,setOpen]=useState(false);const [position,setPosition]=useState({left:12,top:0});
 function show(){const rect=button.current?.getBoundingClientRect();if(rect)setPosition({left:Math.max(12,Math.min(rect.left,innerWidth-268)),top:Math.min(rect.bottom+8,Math.max(12,innerHeight-170))});setOpen(true)}
 return <span className="inline-flex shrink-0 items-center" onMouseEnter={show} onMouseLeave={()=>setOpen(false)}><button ref={button} type="button" aria-label={`Help: ${label}`} aria-describedby={open?id:undefined} aria-expanded={open} onFocus={show} onClick={show} onBlur={()=>setOpen(false)} onKeyDown={e=>{if(e.key==='Escape')setOpen(false)}} className="inline-flex h-6 w-6 items-center justify-center rounded-full text-text-muted hover:text-accent focus-visible:outline focus-visible:outline-accent"><svg aria-hidden="true" viewBox="0 0 24 24" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.7"><circle cx="12" cy="12" r="9"/><path d="M9.5 9a2.5 2.5 0 0 1 5 0c0 2-2.5 2-2.5 4m0 3h.01"/></svg></button>{open&&createPortal(<span id={id} role="tooltip" style={position} className="pointer-events-none fixed z-50 w-64 max-w-[calc(100vw-1.5rem)] rounded-md border border-line bg-panel-raised p-3 text-xs font-normal leading-relaxed text-text shadow-xl">{children}</span>,document.body)}</span>;
}
