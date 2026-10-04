import {describe,it,expect} from 'vitest';
import {servingModel,deploymentRows,placedSpec} from './managementModel';
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

it('shows the same physical-device CUDA mask as the rendered Fleet manifest',()=>{
 const spec={gpus:'device=0',environment:{NVIDIA_VISIBLE_DEVICES:'all',CUDA_VISIBLE_DEVICES:'0',KEEP:'yes'}};
 const placed=placedSpec(spec,{placement:{group_id:'g',rank:0,addresses:['gpu01'],gpus:[3],gpu_uuids:['GPU-identity']}});
 expect(placed.gpus).toBe('device=3');
 expect(placed.environment).toEqual({NVIDIA_VISIBLE_DEVICES:'3',CUDA_VISIBLE_DEVICES:'GPU-identity',KEEP:'yes'});
 expect(spec.environment.CUDA_VISIBLE_DEVICES).toBe('0');
});
