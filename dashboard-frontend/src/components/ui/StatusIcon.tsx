/** Compact state indicators keep their meaning available to assistive technology. */
export function StatusIcon({status}:{status:string}) {
  const good=['ready','live','completed','validated','accepting'].includes(status);const inactive=['unloaded','off','disabled','cancelled','canceled'].includes(status);const failed=['failed','error','down'].includes(status);
  return <span role="img" aria-label={status} title={status} className={`inline-flex items-center justify-center rounded-full p-1 ${good?'text-status-healthy bg-status-healthy/10':failed?'text-status-down bg-status-down/10':inactive?'text-text-muted bg-line':'text-status-cooling bg-status-cooling/10'}`}><svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true">{good?<path d="m3 8 3 3 7-7"/>:failed?<><path d="m4 4 8 8M12 4l-8 8"/></>:inactive?<path d="M4 8h8"/>:<><circle cx="8" cy="8" r="6"/><path d="M8 4v4l3 2"/></>}</svg></span>;
}
