import {it,expect} from 'vitest';
import {windowToDraft,windowFromDraft} from './accessEditModel';
it('round trips recurring, overnight and absolute UTC windows without dropping precision',()=>{
 const windows=[{weekday_mask:0,start_minute:0,end_minute:0,absolute_start_ms:null,absolute_end_ms:null},{weekday_mask:65,start_minute:1320,end_minute:370,absolute_start_ms:1792334310123,absolute_end_ms:1792444310789}];
 expect(windows.map(windowToDraft).map(windowFromDraft)).toEqual(windows);
 expect(windowToDraft(windows[1]!).days).toEqual(['Monday','Sunday']);
});
