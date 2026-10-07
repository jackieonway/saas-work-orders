import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { adminConfig, ManagementApi, parseArgs, run, SafeAdminError, validateRelease } from '../platform-admin-core.mjs';

const ID = { app:'10000000-0000-4000-8000-000000000001', client:'10000000-0000-4000-8000-000000000002', root:'10000000-0000-4000-8000-000000000003', release:'10000000-0000-4000-8000-000000000004', previous:'10000000-0000-4000-8000-000000000005', foreign:'10000000-0000-4000-8000-000000000006' };
const TOKEN = 'test-administrator-bearer-private';
const SECRET = 'test-server-only-secret-private';
const CLIENT_ID = 'work-orders-client-platform-generated';
const definitions = JSON.parse(readFileSync(new URL('../bootstrap.manifest.json', import.meta.url), 'utf8'));
const app = () => ({id:ID.app, ...definitions.application});
const client = () => ({id:ID.client,clientId:CLIENT_ID,...definitions.oauthClient,applicationId:null,status:'ENABLED'});
const root = () => ({id:ID.root,applicationId:ID.app,parentId:null,linkUrl:'',...definitions.menus[0],children:[]});
const release = () => ({id:ID.release,applicationId:ID.app,environment:'production',version:'1.0.0',entryType:'HTML',entryUrl:'/sub-apps/work-orders/versions/1.0.0/index.html',resources:[],validationStatus:'DRAFT',active:false});
const deployment = () => ({applicationId:ID.app,environment:'production',activeReleaseId:ID.release,previousReleaseId:ID.previous,version:'1.0.0',entryType:'HTML'});
const artifact = () => ({environment:'production',version:'1.0.0',entryType:'HTML',entryUrl:'/sub-apps/work-orders/versions/1.0.0/index.html',resources:[{url:'./assets/main.js',kind:'SCRIPT'}]});
const settings = {environment:'development',allowLoopbackHttp:true};
const env = origin => ({PLATFORM_API_ORIGIN:origin,PLATFORM_ADMIN_BEARER:TOKEN,PLATFORM_CLIENT_ID:CLIENT_ID,PLATFORM_CLIENT_SECRET:SECRET});

async function fixture(t, handler, apiOptions) {
  const calls = [];
  const sockets = new Set();
  const server = createServer(async (req,res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const text = Buffer.concat(chunks).toString('utf8');
    const call = {method:req.method,path:req.url,authorization:req.headers.authorization,body:text ? JSON.parse(text) : undefined};
    calls.push(call);
    assert.equal(call.authorization, `Bearer ${TOKEN}`);
    res.setHeader('content-type','application/json');
    try {
      const result = await handler(call,req,res);
      if (!res.writableEnded && result !== undefined) res.end(JSON.stringify(result));
    } catch {
      res.statusCode=500;
      if (!res.writableEnded) res.end(JSON.stringify({message:'fixture handler failed'}));
    }
  });
  server.on('connection', socket => {sockets.add(socket);socket.on('close',()=>sockets.delete(socket));});
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
  t.after(async () => {for(const socket of sockets)socket.destroy();await new Promise(resolve=>server.close(resolve));});
  const origin = `http://127.0.0.1:${server.address().port}`;
  return {api:new ManagementApi(adminConfig(env(origin),settings),apiOptions),calls,origin};
}

function rejectsCategory(promise,category) {
  return assert.rejects(promise,error=>error instanceof SafeAdminError && error.category===category);
}

test('default plan/help need no env and cannot mutate remotely',async()=>{
  const api={request(){assert.fail('network called');}};
  const result=await run([],{},api);
  assert.equal(result.mode,'PLAN');assert.deepEqual(result.grants,[]);assert.equal(result.externalWrites,false);
  assert.match(await run(['--help'],{},api),/--apply/);
  for(const args of [['bootstrap'],['rollback'],['preflight','--release-id',ID.release],['activate','--release-id',ID.release]]){
    const planned=await run(args,{},api);assert.equal(planned.mode,'PLAN');assert.equal(planned.externalWrites,false);
  }
});

test('arguments cannot carry credentials, injected IDs, unknown flags, or accidental writes',()=>{
  for(const args of [['--token',TOKEN],['bootstrap','--apply','--apply'],['plan','--apply'],['inspect','--apply'],['activate'],['rollback','--release-id',ID.release],['bootstrap','--app-id',"a' OR 1=1"],['rollback','--environment','../../oops'],['--help','--apply']]){
    assert.throws(()=>parseArgs(args),error=>error instanceof SafeAdminError);
  }
});

test('production requires HTTPS and development HTTP is explicit literal loopback only',()=>{
  for(const origin of ['http://127.0.0.1:3001','http://localhost:3001','http://[::1]:3001']) {
    assert.throws(()=>adminConfig(env(origin),{environment:'production',allowLoopbackHttp:true}),/HTTPS_REQUIRED/);
    assert.throws(()=>adminConfig(env(origin),{environment:'development',allowLoopbackHttp:false}),/HTTPS_REQUIRED/);
    assert.equal(adminConfig(env(origin),settings).origin,origin);
  }
  for(const origin of ['http://example.com','https://user:pass@example.com','https://example.com/path','https://example.com/?token=hidden','https://example.com/#token','not-a-url',' https://example.com'])assert.throws(()=>adminConfig(env(origin),settings));
  assert.equal(adminConfig(env('https://platform.example.com'),{environment:'production'}).origin,'https://platform.example.com');
  assert.throws(()=>adminConfig({...env('https://platform.example.com'),PLATFORM_ADMIN_BEARER:'a\r\nb'},{}),/ADMIN_BEARER_REQUIRED/);
});

test('HTTP 302 never follows or forwards Bearer to the redirect target',async t=>{
  let redirected=0;
  const target=createServer((req,res)=>{redirected++;res.end('leaked');});
  await new Promise(resolve=>target.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>target.close(resolve)));
  const {api,calls}=await fixture(t,(call,req,res)=>{res.statusCode=302;res.setHeader('location',`http://127.0.0.1:${target.address().port}/secret-target`);return {message:TOKEN};});
  await rejectsCategory(api.request('GET','/api/applications'),'REDIRECT_REJECTED');
  assert.equal(redirected,0);assert.equal(calls.length,1);
});

test('HTTP errors omit untrusted response body and do not retry',async t=>{
  const {api,calls}=await fixture(t,(call,req,res)=>{res.statusCode=503;return {message:`${TOKEN} ${SECRET} https://secret.invalid`};});
  await assert.rejects(api.request('POST','/api/applications',{}),error=>error.category==='HTTP_ERROR'&&error.status===503&&error.message==='HTTP_ERROR');
  assert.equal(calls.length,1);
});

test('timeout covers unfinished response bodies and mutation is not retried',async t=>{
  const {api,calls}=await fixture(t,(call,req,res)=>{res.write('{"pending":');},{timeoutMs:60});
  await rejectsCategory(api.request('POST','/api/applications',{}),'REQUEST_TIMEOUT');assert.equal(calls.length,1);
});

test('oversized JSON and invalid MIME/JSON are safely rejected',async t=>{
  const {api}=await fixture(t,(call,req,res)=>{
    if(call.path.endsWith('large'))return {large:'x'.repeat(1024)};
    if(call.path.endsWith('html')){res.setHeader('content-type','text/html');res.end(`<html>${TOKEN}</html>`);return;}
    res.end(`not JSON ${TOKEN}`);
  },{maxBytes:128});
  await rejectsCategory(api.request('GET','/api/large'),'RESPONSE_TOO_LARGE');
  for(const path of ['/api/html','/api/bad'])await rejectsCategory(api.request('GET',path),'INVALID_RESPONSE');
  await rejectsCategory(api.request('GET','https://other.invalid/api/applications'),'INVALID_REQUEST');
});

test('bootstrap requires explicit existing client before creating anything',async t=>{
  const {api,calls}=await fixture(t,call=>call.path==='/api/applications'?[]:[]);
  await rejectsCategory(run(['bootstrap','--apply'],env('ignored'),api),'CLIENT_CONFLICT');
  assert.ok(calls.every(call=>call.method==='GET'));
});

test('foreign app/client and duplicate identity conflict before any mutation',async t=>{
  let mode='foreign';
  const {api,calls}=await fixture(t,call=>{
    if(call.path==='/api/applications')return mode==='duplicate'?[app(),app()]:[app()];
    if(call.path==='/api/oauth/clients')return [{...client(),applicationId:ID.foreign}];
    assert.fail('unexpected call');
  });
  await rejectsCategory(run(['bootstrap','--apply'],env('ignored'),api),'CLIENT_OWNERSHIP_CONFLICT');
  mode='duplicate';await rejectsCategory(run(['bootstrap','--apply'],env('ignored'),api),'APPLICATION_CONFLICT');
  assert.ok(calls.every(call=>call.method==='GET'));
});

test('TRUSTED mode and OAuth scope widening are never silently rewritten',async t=>{
  let mode='trusted';
  const {api,calls}=await fixture(t,call=>{
    if(call.path==='/api/applications')return [{...app(),runtimeMode:mode==='trusted'?'TRUSTED':'IFRAME'}];
    if(call.path==='/api/oauth/clients')return [{...client(),scopes:['openid','system:admin']}];
    assert.fail('unexpected call');
  });
  await rejectsCategory(run(['bootstrap','--apply'],env('ignored'),api),'APPLICATION_CONFLICT');
  mode='scope';await rejectsCategory(run(['bootstrap','--apply'],env('ignored'),api),'CLIENT_CONFLICT');
  assert.ok(calls.every(call=>call.method==='GET'));
});

test('partial bootstrap resumes without rewriting existing menus or grants',async t=>{
  const state={applications:[],client:client(),menus:[],sequence:10};
  const {api,calls}=await fixture(t,call=>{
    if(call.method==='GET'&&call.path==='/api/applications')return state.applications;
    if(call.method==='GET'&&call.path==='/api/oauth/clients')return [state.client];
    if(call.method==='POST'&&call.path==='/api/applications'){
      assert.equal(call.body.autoCreateMenu,false);assert.equal(call.body.runtimeMode,'IFRAME');assert.equal(call.body.entryType,'HTML');
      state.applications=[{id:ID.app,...call.body}];return state.applications[0];
    }
    if(call.method==='PUT'&&call.path===`/api/oauth/clients/${ID.client}`){
      assert.deepEqual(call.body,{applicationId:ID.app});state.client={...state.client,...call.body};return state.client;
    }
    if(call.method==='GET'&&call.path===`/api/menus/tree?applicationId=${ID.app}`)return {menus:state.menus};
    if(call.method==='POST'&&call.path==='/api/menus'){
      const id=`10000000-0000-4000-8000-${String(state.sequence++).padStart(12,'0')}`;
      const row={id,...call.body};state.menus.push(row);return row;
    }
    assert.fail('unexpected mutation');
  });
  const first=await run(['bootstrap','--apply'],env('ignored'),api);
  assert.equal(first.created.length,9);assert.equal(Object.keys(first.menuIds).length,7);
  const boundary=calls.length;
  const second=await run(['bootstrap','--apply','--app-id',ID.app],env('ignored'),api);
  assert.deepEqual(second.created,[]);assert.ok(calls.slice(boundary).every(call=>call.method==='GET'));
  assert.equal(state.menus.length,7);assert.deepEqual(second.grants,[]);assert.equal(second.releaseActivated,false);
  assert.ok(calls.every(call=>!call.path.includes('authorizations')&&!call.path.includes('roles')&&!call.path.includes('memberships')));
  assert.ok(!JSON.stringify([first,second]).includes(TOKEN));assert.ok(!JSON.stringify([first,second]).includes(SECRET));
});

test('duplicate or foreign menu ownership blocks bootstrap before association',async t=>{
  let mode='duplicate';
  const {api,calls}=await fixture(t,call=>{
    if(call.path==='/api/applications')return [app()];
    if(call.path==='/api/oauth/clients')return [client()];
    if(call.path.startsWith('/api/menus/tree'))return {menus:mode==='duplicate'?[root(),{...root(),id:ID.foreign}]:[{...root(),applicationId:ID.foreign}]};
    assert.fail('mutation should not happen');
  });
  await rejectsCategory(run(['bootstrap','--apply'],env('ignored'),api),'MENU_CONFLICT');
  mode='foreign';await rejectsCategory(run(['bootstrap','--apply'],env('ignored'),api),'MENU_OWNERSHIP_CONFLICT');
  assert.ok(calls.every(call=>call.method==='GET'));
});

test('a noncanonical menu create response stops subsequent writes and is not reported as success',async t=>{
  const {api,calls}=await fixture(t,call=>{
    if(call.path==='/api/applications')return [app()];
    if(call.path==='/api/oauth/clients')return [{...client(),applicationId:ID.app}];
    if(call.path.startsWith('/api/menus/tree'))return {menus:[root()]};
    if(call.method==='POST'&&call.path==='/api/menus')return {id:ID.foreign,...call.body,permission:'other:permission'};
    assert.fail('unexpected request');
  });
  await rejectsCategory(run(['bootstrap','--apply'],env('ignored'),api),'MENU_CONFLICT');
  assert.equal(calls.filter(call=>call.method==='POST').length,1);
});

test('release input forbids cross-app paths, traversal, ESM, mutable resources and secrets',()=>{
  assert.deepEqual(validateRelease(artifact(),{environment:'production'}),artifact());
  const bad=[{...artifact(),entryType:'ESM'},{...artifact(),runtimeMode:'TRUSTED'},{...artifact(),entryUrl:'/sub-apps/other/versions/1.0.0/index.html'},{...artifact(),version:'../1'},{...artifact(),entryUrl:'https://user:pass@example.com/sub-apps/work-orders/versions/1.0.0/index.html'},{...artifact(),resources:[{url:'/latest.js',kind:'SCRIPT'}]},{...artifact(),resources:[{url:'../../evil.js',kind:'SCRIPT'}]},{...artifact(),resources:[{url:'./main.js?token=private',kind:'SCRIPT'}]},{...artifact(),resources:[{url:'https://other.invalid/assets/main.js',kind:'SCRIPT'}]},{...artifact(),clientSecret:SECRET},{...artifact(),environment:'development'}];
  for(const value of bad)assert.throws(()=>validateRelease(value,{environment:'production'}),error=>error instanceof SafeAdminError);
  for(const encoded of ['%2f..%2f..%2fother','%5c..%5cother','%252fother','%00hidden','%invalid']){
    assert.throws(()=>validateRelease({...artifact(),entryUrl:`/sub-apps/work-orders/versions/1.0.0/${encoded}/index.html`},{environment:'production'}),error=>error instanceof SafeAdminError);
    assert.throws(()=>validateRelease({...artifact(),resources:[{url:`./${encoded}/main.js`,kind:'SCRIPT'}]},{environment:'production'}),error=>error instanceof SafeAdminError);
  }
});

test('register reads actual package release, remains plan until apply, and never replaces a version',async t=>{
  const directory=mkdtempSync(join(tmpdir(),'work-orders-admin-test-'));t.after(()=>rmSync(directory,{recursive:true,force:true}));
  const input=join(directory,'dist');const output=join(directory,'archive');mkdirSync(join(input,'assets'),{recursive:true});
  writeFileSync(join(input,'index.html'),'<html><script type="module" src="./assets/main.js"></script></html>');
  writeFileSync(join(input,'assets','main.js'),'document.body.dataset.ready="true";');
  const packager=fileURLToPath(new URL('../../../../scripts/package-microapp.mjs',import.meta.url));
  const packaged=spawnSync(process.execPath,[packager,'--app','work-orders','--environment','production','--version','1.0.0','--input',input,'--output-root',output],{encoding:'utf8'});
  assert.equal(packaged.status,0);
  const file=join(output,'work-orders','versions','1.0.0','release.json');
  const packagedArtifact=JSON.parse(readFileSync(file,'utf8'));validateRelease(packagedArtifact,{environment:'production'});
  let registered=false;
  const {api,calls}=await fixture(t,call=>{
    if(call.path==='/api/applications')return [app()];
    if(call.method==='GET'&&call.path.includes('/releases?'))return registered?[release()]:[];
    if(call.method==='POST'&&call.path.endsWith('/releases')){assert.deepEqual(call.body,packagedArtifact);registered=true;return release();}
    assert.fail('unexpected activation/update');
  });
  const planned=await run(['register','--release-file',file],{},api);assert.equal(planned.mode,'PLAN');assert.equal(calls.length,0);
  const result=await run(['register','--release-file',file,'--apply'],env('ignored'),api);assert.equal(result.releaseActivated,false);
  const boundary=calls.length;
  await rejectsCategory(run(['register','--release-file',file,'--apply'],env('ignored'),api),'VERSION_EXISTS');
  assert.ok(calls.slice(boundary).every(call=>call.method==='GET'));assert.equal(calls.filter(call=>call.method==='POST').length,1);
});

test('HTTP 200 FAILED preflight is a failure and activation is blocked without PASSED',async t=>{
  const {api,calls}=await fixture(t,call=>{
    if(call.path==='/api/applications')return [app()];
    if(call.method==='GET'&&call.path.includes('/releases?'))return [release()];
    if(call.path.endsWith('/preflight'))return {...release(),validationStatus:'FAILED',validationResult:{message:TOKEN,url:'https://private.invalid'}};
    assert.fail('activation not authorized');
  });
  await rejectsCategory(run(['preflight','--release-id',ID.release,'--apply'],env('ignored'),api),'PREFLIGHT_FAILED');
  const boundary=calls.length;
  await rejectsCategory(run(['activate','--release-id',ID.release,'--apply'],env('ignored'),api),'PREFLIGHT_REQUIRED');
  assert.ok(calls.slice(boundary).every(call=>call.method==='GET'));
});

test('preflight, activation and rollback are separate explicit mutations in one application',async t=>{
  const {api,calls}=await fixture(t,call=>{
    if(call.path==='/api/applications')return [app()];
    if(call.method==='GET'&&call.path.includes('/releases?'))return [{...release(),validationStatus:'PASSED'}];
    if(call.path.endsWith('/preflight'))return {...release(),validationStatus:'PASSED'};
    if(call.path.endsWith('/activate'))return deployment();
    if(call.path.endsWith('/deployments'))return [deployment()];
    if(call.path.endsWith('/rollback'))return {...deployment(),activeReleaseId:ID.previous,previousReleaseId:ID.release,version:'0.9.0'};
    assert.fail('unexpected operation');
  });
  await run(['preflight','--release-id',ID.release,'--apply'],env('ignored'),api);
  assert.equal(calls.filter(call=>call.method==='POST').length,1);
  await run(['activate','--release-id',ID.release,'--apply'],env('ignored'),api);
  await run(['rollback','--apply'],env('ignored'),api);
  assert.deepEqual(calls.filter(call=>call.method==='POST').map(call=>call.path),[`/api/applications/${ID.app}/releases/${ID.release}/preflight`,`/api/applications/${ID.app}/releases/${ID.release}/activate`,`/api/applications/${ID.app}/deployments/production/rollback`]);
});

test('foreign release id and missing previous deployment do not cause writes',async t=>{
  const {api,calls}=await fixture(t,call=>{
    if(call.path==='/api/applications')return [app()];
    if(call.path.includes('/releases?'))return [{...release(),validationStatus:'PASSED'}];
    if(call.path.endsWith('/deployments'))return [{...deployment(),previousReleaseId:null}];
    assert.fail('write not expected');
  });
  await rejectsCategory(run(['activate','--release-id',ID.foreign,'--apply'],env('ignored'),api),'RELEASE_NOT_FOUND');
  await rejectsCategory(run(['rollback','--apply'],env('ignored'),api),'ROLLBACK_UNAVAILABLE');
  assert.ok(calls.every(call=>call.method==='GET'));
});

test('CLI safe errors never echo credentials, URL, response text, args or stacks',()=>{
  const cli=fileURLToPath(new URL('../platform-admin.mjs',import.meta.url));
  for(const args of [['unexpected-'+TOKEN],['inspect'],['register','--release-file',`https://private.invalid/${SECRET}`,'--apply']]){
    const result=spawnSync(process.execPath,[cli,...args],{encoding:'utf8',env:{...process.env,...env(`https://user:${SECRET}@private.invalid`)}});
    assert.equal(result.status,1);
    const output=result.stdout+result.stderr;
    assert.ok(!output.includes(TOKEN));assert.ok(!output.includes(SECRET));assert.ok(!output.includes('private.invalid'));assert.ok(!output.includes(' at '));
    assert.equal(JSON.parse(result.stderr).ok,false);
  }
});
