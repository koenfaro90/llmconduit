import {describe,it,expect} from 'vitest';
import {validCheckpoint,isPreparationRun} from './preparationModel';
describe('checkpoint subfolder',()=>{
 it('keeps legacy requirements and accepts explicit pipeline folders',()=>{
  const base={repository:'MiniMaxAI/MiniMax-H3',revision:'a'.repeat(40)};
  expect(validCheckpoint(base)).toBe(true);
  expect(validCheckpoint({...base,subfolder:'FL2VA/text_encoder'})).toBe(true);
  for(const subfolder of ['/FL2VA','../FL2VA','FL2VA/../Ref2VA','FL2VA/','FL2VA//weights'])expect(validCheckpoint({...base,subfolder})).toBe(false);
 });
 it('preserves a selected folder on a preparation response',()=>{
  const run={id:'test',endpoint_id:'host',recipe_id:'h3',revision_digest:'digest',image:'image',created_at_ms:1,rank:0,cancel_requested:false,report:null,checkpoints:[{repository:'MiniMaxAI/MiniMax-H3',revision:'a'.repeat(40),subfolder:'FL2VA'}]};
  const wire=JSON.parse(JSON.stringify(run));expect(isPreparationRun(wire)).toBe(true);expect(wire.checkpoints[0].subfolder).toBe('FL2VA');
 });
});
