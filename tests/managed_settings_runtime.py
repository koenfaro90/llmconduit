#!/usr/bin/env python3
"""Exercise the real supervisor with private temporary state and a fake upstream.

Build the release binary, then run python3 tests/managed_settings_runtime.py.
TEST_BINARY can select another build. No production configuration is read.
"""
import sqlite3,pathlib,tempfile,subprocess,os,json,time,http.cookiejar,urllib.request,urllib.error,socket,secrets,threading,http.server,uuid
BINARY=os.environ.get('TEST_BINARY',str(pathlib.Path(__file__).resolve().parents[1]/'target/release/llmconduit'))
class Backend(http.server.BaseHTTPRequestHandler):
 def log_message(self,*a):pass
 def do_GET(self):
  data=json.dumps({'object':'list','data':[{'id':'test-model'}]}).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(data)
 def do_POST(self):
  self.rfile.read(int(self.headers.get('Content-Length','0')));self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
  for chunk in [{'id':'test','choices':[{'index':0,'delta':{'role':'assistant','content':'ok'},'finish_reason':None}]},{'id':'test','choices':[{'index':0,'delta':{},'finish_reason':'stop'}],'usage':{'prompt_tokens':1,'completion_tokens':1,'total_tokens':2}}]:self.wfile.write(('data: '+json.dumps(chunk)+'\n\n').encode())
  self.wfile.write(b'data: [DONE]\n\n')
def port():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
with tempfile.TemporaryDirectory(prefix='llm-settings-e2e-') as d:
 root=pathlib.Path(d);serveport=port();base='http://127.0.0.1:'+str(serveport);backend=http.server.ThreadingHTTPServer(('127.0.0.1',0),Backend);threading.Thread(target=backend.serve_forever,daemon=True).start()
 ident=lambda:str(uuid.uuid4());bid,pid,aid=ident(),ident(),ident()
 config={'bind_addr':'0.0.0.0:'+str(serveport),'upstream_base_url':'http://127.0.0.1:'+str(backend.server_port)+'/v1','upstream_model':'test-model','auth':{'mode':'disabled','store_path':str(root/'policy.db')},'control_plane':{'storage':{'backend':'sqlite','url':'sqlite:'+str(root/'history.db')},'auth':{'require':True},'operational':{'backends':[{'id':bid,'name':'local-test','enabled':True,'base_url':'http://127.0.0.1:'+str(backend.server_port)+'/v1'}],'model_profiles':[{'id':pid,'name':'test-model','backends':[bid],'upstream_model':'test-model'}],'aliases':[{'id':aid,'name':'public-model','profiles':[pid]}]}}}
 path=root/'bootstrap.json';path.write_text(json.dumps(config));env={k:v for k,v in os.environ.items() if not k.startswith('LLMCONDUIT_') and k not in ['OPENAI_API_KEY','BRAVE_SEARCH_API_KEY']};password=secrets.token_hex(20);env.update({'LLMCONDUIT_ADMIN_USERNAME':'operator','LLMCONDUIT_ADMIN_PASSWORD':password,'LLMCONDUIT_DASHBOARD_SESSION_KEY':secrets.token_hex(32),'LLMCONDUIT_DASHBOARD_PUBLIC_ORIGIN':base,'LLMCONDUIT_DASHBOARD_ALLOW_MUTATIONS':'1','LLMCONDUIT_ALLOW_INSECURE_DASHBOARD':'1','LLMCONDUIT_REQUEST_TIMEOUT_SECS':'600','LLMCONDUIT_MANAGED_INGRESS_FILE':str(root/'ingress.yaml'),'RUST_LOG':'error'})
 log=(root/'log').open('w');proc=subprocess.Popen([BINARY,'supervise','--config',str(path),'--state-dir',str(root/'managed'),'--with-debug-ui'],env=env,stdout=log,stderr=log)
 jar=http.cookiejar.CookieJar();opener=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
 def call(path,method='GET',body=None,csrf=True):
  headers={'Origin':base,'Content-Type':'application/json'}
  if csrf:
   token=next((c.value for c in jar if 'csrf' in c.name),'');headers['X-CSRF-Token']=token
  req=urllib.request.Request(base+path,method=method,data=None if body is None else json.dumps(body).encode(),headers=headers)
  with opener.open(req,timeout=5) as response:return json.load(response)
 def wait(fn,seconds=65):
  until=time.monotonic()+seconds
  while time.monotonic()<until:
   try:
    result=fn()
    if result:return result
   except (OSError,urllib.error.HTTPError,ValueError):pass
   if proc.poll() is not None:raise RuntimeError('supervisor exited; '+(root/'log').read_text()[-1000:])
   time.sleep(.2)
  raise AssertionError('timed out waiting for supervisor')
 def stage(scope,values,environment={}):
  v=call('/dashboard/api/settings/'+scope);return call('/dashboard/api/settings/'+scope,'POST',{'revision':v['revision'],'values':values,'environment':environment})
 def apply(v,status='applied'):
  call('/dashboard/api/settings/apply','POST',{'revision':v['revision']});return wait(lambda:(lambda x:x if x['status']==status and (status!='applied' or x['applied_revision']==v['revision']) else None)(call('/dashboard/api/settings/gateway')))
 try:
  wait(lambda:call('/dashboard/login','POST',{'username':'operator','password':password}))
  for scope in ['gateway','profiles','providers','integrations','authentication','access','observability','service']:
   v=call('/dashboard/api/settings/'+scope);assert v['revision']==0;assert password not in json.dumps(v)
  try:stagev=call('/dashboard/api/settings/gateway','POST',{'revision':0,'values':{},'environment':{}},csrf=False);raise AssertionError('missing CSRF accepted')
  except urllib.error.HTTPError as e:assert e.code==403
  aliases=call('/dashboard/api/aliases')['aliases'];assert aliases[0]['name']=='public-model'
  key=call('/dashboard/api/keys','POST',{'label':'migration-test','allowed_models':['public-model']})
  raw=key.get('secret') or key.get('api_key');assert isinstance(raw,str),key.keys()
  def infer(model='public-model',expected=200):
   req=urllib.request.Request(base+'/v1/chat/completions',data=json.dumps({'model':model,'messages':[{'role':'user','content':'test'}],'max_tokens':8}).encode(),headers={'Authorization':'Bearer '+raw,'Content-Type':'application/json'})
   try:
    with urllib.request.urlopen(req,timeout=8) as r:assert r.status==expected;assert json.load(r)['choices'][0]['message']['content']=='ok'
   except urllib.error.HTTPError as e:assert e.code==expected
  infer()
  v=stage('gateway',{'/request_timeout_secs':120},{'LLMCONDUIT_REQUEST_TIMEOUT_SECS':None});apply(v);assert call('/dashboard/api/settings/gateway')['effective_values']['/request_timeout_secs']==120
  # A validated listener whose socket is occupied must roll back, retaining login/key/alias state.
  blocker=socket.socket();blocker.bind(('127.0.0.1',0));blocker.listen();v=stage('service',{'/bind_addr':'127.0.0.1:'+str(blocker.getsockname()[1])});apply(v,'rolled_back');blocker.close();infer();assert call('/dashboard/api/aliases')['aliases']==aliases
  v=stage('service',{'/bind_addr':config['bind_addr']});apply(v)
  v=stage('service',{'/bind_addr':'127.0.0.1:'+str(serveport)});apply(v,'rolled_back');infer()
  v=stage('service',{'/bind_addr':config['bind_addr']});apply(v)
  v=stage('access',{'/auth':{'mode':'enforce','store_path':str(root/'policy.db')}});apply(v);infer();infer('test-model',403);assert call('/dashboard/api/auth/summary')['enabled'] is True
  v=stage('access',{'/auth':{'mode':'enforce','store_path':str(root/'moved-policy.db')}});apply(v);infer();infer('test-model',403)
  # Native SQLite migration preserves existing SQL client keys, dashboard users and aliases.
  v=stage('service',{'/control_plane/storage/url':'sqlite:'+str(root/'new-history.db')});apply(v);infer();assert call('/dashboard/api/aliases')['aliases']==aliases
  v=stage('service',{'/control_plane/storage/backend':'none'});apply(v);infer();assert call('/dashboard/api/aliases')['aliases']==aliases
  v=stage('service',{'/control_plane/storage/backend':'sqlite'});apply(v);infer()
  proc.terminate();proc.wait(timeout=45);proc=subprocess.Popen([BINARY,'supervise','--config',str(path),'--state-dir',str(root/'managed'),'--with-debug-ui'],env=env,stdout=log,stderr=log);wait(lambda:call('/dashboard/api/settings/gateway'));assert call('/dashboard/api/settings/gateway')['effective_values']['/request_timeout_secs']==120;infer()
  proc.terminate();proc.wait(timeout=45)
  # Seed the old SQL operational format to verify one-time import and that it
  # cannot silently override subsequent managed profile revisions.
  legacy=json.loads(json.dumps(config['control_plane']['operational']));legacy['model_profiles'][0]['name']='legacy-profile'
  with sqlite3.connect(root/'history.db') as db:
   db.execute("DELETE FROM settings WHERE key='gateway_settings_v1'")
   db.execute("INSERT INTO settings(key,value) VALUES('operational',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",(json.dumps(legacy),))
  proc=subprocess.Popen([BINARY,'supervise','--config',str(path),'--state-dir',str(root/'managed'),'--with-debug-ui'],env=env,stdout=log,stderr=log)
  wait(lambda:call('/dashboard/api/settings/profiles'))
  profileview=call('/dashboard/api/settings/profiles');profiles=profileview['values']['/control_plane/operational/model_profiles'];assert profiles[0]['name']=='legacy-profile'
  profiles[0]['name']='managed-profile';v=stage('profiles',{'/control_plane/operational/model_profiles':profiles});apply(v)
  assert call('/dashboard/api/settings/profiles')['active_values']['/control_plane/operational/model_profiles'][0]['name']=='managed-profile'
  infer()
  names=[m['id'] for m in call('/dashboard/api/catalog')];assert 'managed-profile' in names and 'legacy-profile' not in names,names
  print('Native supervisor: 8 pages, CSRF, exact environment apply, rollback, preserved login/aliases/keys, policy migration, moved policy store and scope, SQL database move, history off/on, restart persistence, legacy SQL import/edit PASS',flush=True)
 finally:
  proc.terminate()
  try:proc.wait(timeout=45)
  except subprocess.TimeoutExpired:proc.kill();proc.wait()
  backend.shutdown();log.close()
