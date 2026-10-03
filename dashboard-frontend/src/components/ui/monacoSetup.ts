import * as monaco from 'monaco-editor/editor/editor.api.js';
import {jsonDefaults} from 'monaco-editor/languages/features/json/register.js';
import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker';
import JsonWorker from 'monaco-editor/language/json/json.worker.js?worker';
self.MonacoEnvironment={getWorker:(_moduleId,label)=>label==='json'?new JsonWorker():new EditorWorker()};
jsonDefaults.setDiagnosticsOptions({validate:true,allowComments:false,enableSchemaRequest:false,schemas:[{uri:'llmconduit://fleet-recipe',fileMatch:['*'],schema:{type:'object',properties:{image:{type:'string'},command:{type:'array',items:{type:'string'}},environment:{type:'object',additionalProperties:{type:'string'}},memory_limit:{type:'string'},user:{type:'string'},ports:{type:'array',items:{type:'object'}},mounts:{type:'array',items:{type:'object'}}}}}]});
export const editor=monaco.editor;
