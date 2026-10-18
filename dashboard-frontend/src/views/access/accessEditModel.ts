import type {AuthPolicyTimeWindow} from '../../api/types';
import type {Json} from '../settings/settingsModel';
export const weekDays=['Monday','Tuesday','Wednesday','Thursday','Friday','Saturday','Sunday'];
const time=(minutes:number)=>`${String(Math.floor(minutes/60)).padStart(2,'0')}:${String(minutes%60).padStart(2,'0')}`;
const minute=(value:Json|undefined)=>{const [h,m]=String(value??'00:00').split(':').map(Number);return h!*60+m!;};
export function windowToDraft(w:AuthPolicyTimeWindow):Record<string,Json>{return {
 days:weekDays.filter((_,i)=>(w.weekday_mask&(1<<i))!==0),start_time:time(w.start_minute),end_time:time(w.end_minute),
 absolute_start:w.absolute_start_ms===null?null:new Date(w.absolute_start_ms).toISOString().slice(0,-1),
 absolute_end:w.absolute_end_ms===null?null:new Date(w.absolute_end_ms).toISOString().slice(0,-1),
};}
export function windowFromDraft(w:Record<string,Json>):AuthPolicyTimeWindow{return {
 weekday_mask:(w.days as string[]??[]).reduce((mask,day)=>mask|(1<<weekDays.indexOf(day)),0),start_minute:minute(w.start_time),end_minute:minute(w.end_time),
 absolute_start_ms:w.absolute_start?Date.parse(String(w.absolute_start)+'Z'):null,
 absolute_end_ms:w.absolute_end?Date.parse(String(w.absolute_end)+'Z'):null,
};}
