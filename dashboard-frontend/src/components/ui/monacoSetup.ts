// Initialize contributions before the first editor: JSON mode also loads them,
// and late singleton registration leaves subsequent editors with missing services.
import 'monaco-editor/editor/editor.main.js';
import * as monaco from 'monaco-editor/editor/editor.api.js';
import {jsonDefaults} from 'monaco-editor/languages/features/json/register.js';
import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker';
import JsonWorker from 'monaco-editor/language/json/json.worker.js?worker';
self.MonacoEnvironment={getWorker:(_moduleId,label)=>label==='json'?new JsonWorker():new EditorWorker()};
jsonDefaults.setDiagnosticsOptions({validate:true,allowComments:false,enableSchemaRequest:false,schemas:[{uri:'llmconduit://fleet-recipe',fileMatch:['*'],schema:{type:'object',properties:{metadata:{type:'object',additionalProperties:false,properties:{size_name:{type:'string'},size_source_digest:{type:'string'},memory_estimate_note:{type:'string'},gpu_families:{type:'array',items:{type:'string'}},image_sources:{type:'object',additionalProperties:{type:'string'}},image_source_urls:{type:'object',additionalProperties:{type:'string',format:'uri'}}}},sizes:{type:'object',additionalProperties:{type:'object',properties:{description:{type:'string'},overrides:{type:'object'},vllm:{type:'object'}}}},gpu_memory:{type:'object',additionalProperties:false,required:['mode'],properties:{mode:{enum:['exclusive','shared']},mib_per_device:{type:'integer',minimum:0},fraction:{type:'number',minimum:0,maximum:1}}},image:{type:'string'},command:{type:'array',items:{type:'string'}},environment:{type:'object',additionalProperties:{type:'string'}},memory_limit:{type:'string'},user:{type:'string'},ports:{type:'array',items:{type:'object'}},mounts:{type:'array',items:{type:'object'}}}}}]});
export const editor=monaco.editor;
