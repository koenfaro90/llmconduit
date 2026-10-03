import {describe,it,expect} from 'vitest';
import {servingModel,deploymentRows} from './managementModel';
describe('managed model inventory',()=>{
 it('reads model names from Compose arguments, shell commands and environments',()=>{
  expect(servingModel({command:['repo','--served-model-name','actual']},'fallback')).toBe('actual');
  expect(servingModel({command:['-c','exec vllm serve repo \\ \n--served-model-name "actual" \\ \n--port 8000']},'fallback')).toBe('actual');
  expect(servingModel({environment:{SERVED_MODEL_NAME:'actual'}},'fallback')).toBe('actual');
 });
 it('uses the pinned revision instead of the latest recipe edit',()=>{
  const rows=deploymentRows({recipes:{recipe:[{revision:1,digest:'old',description:'old',created_at_ms:1,model_spec:{command:['--served-model-name','running']}},{revision:2,digest:'new',description:'new',created_at_ms:2,model_spec:{command:['--served-model-name','edited']}}]},deployments:{dep:{endpoint_id:'host',recipe_id:'recipe',revision_digest:'old',generation:1,desired_state:'ready',updated_at_ms:1,observed_at_ms:null,observed:null,error:null}}},[{endpoint_id:'host',label:'gpu01',enabled:true,joined_at_ms:1}]);expect(rows[0]?.model).toBe('running');expect(rows[0]?.host).toBe('gpu01');
 });
});
