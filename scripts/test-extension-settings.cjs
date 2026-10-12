const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const source=fs.readFileSync('browser/chromium/service-worker.js','utf8');
function worker({saved,nativeOk=true}={}){
 let desktopRules;const calls=[],listeners={};const noop=()=>{};const any=new Proxy(noop,{get:()=>any});
 const chrome=new Proxy({}, {get:(o,k)=>o[k]||any});
 chrome.runtime={onInstalled:{addListener:noop},onMessage:{addListener:fn=>listeners.message=fn},getURL:p=>'chrome-extension://test/'+p,sendNativeMessage:(_,payload,cb)=>{if(payload.action==='capture_settings'){if(payload.settings)desktopRules=payload.settings;cb({ok:true,configured:!!desktopRules,settings:desktopRules||{enabled:true,extensions:[]}});return;}calls.push(['native',payload]);if(payload.action==='media_preferences'){cb({ok:true,preferences:{connections:10,maxHeight:1080,container:'mp4'}});return;}cb(nativeOk?{ok:true,id:1}:{ok:false,error:'offline'});}};
 chrome.storage={local:{get:async()=>({captureSettings:saved}),set:async v=>{saved=v.captureSettings}},onChanged:{addListener:noop}};
 chrome.downloads={onCreated:{addListener:fn=>listeners.created=fn},pause:async id=>calls.push(['pause',id]),resume:async id=>calls.push(['resume',id]),cancel:async id=>calls.push(['cancel',id]),erase:async v=>calls.push(['erase',v.id])};
 chrome.cookies={getAll:async()=>[]};chrome.tabs={get:async()=>({url:'https://example.com'}),sendMessage:noop,onRemoved:{addListener:noop},onUpdated:{addListener:noop}};
 const context={chrome,URL,console,setTimeout:noop,self:{navigator:{userAgent:'test'}}};vm.createContext(context);vm.runInContext(source,context);
 return {context,calls,listeners,getSaved:()=>saved};
}
(async()=>{
 const w=worker();await vm.runInContext('captureReady',w.context);
 const save=settings=>new Promise(resolve=>w.listeners.message({type:'popup-control',settings,tabId:1},{url:'chrome-extension://test/popup.html'},resolve));
 await save({enabled:true,groups:['compressed','video'],custom:'.cbz; CBR'});
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/file.zip')",w.context),true);
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/file.mp4')",w.context),true);
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/file.pdf')",w.context),false);
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/file.CBZ')",w.context),true);
 const fresh=worker({saved:w.getSaved()});await vm.runInContext('captureReady',fresh.context);
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/file.pdf')",fresh.context),false);
 await save({enabled:false,groups:['compressed'],custom:''});assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/file.zip')",w.context),false);

 await save({enabled:true,extensions:['.ZIP','zip','CBZ']});
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/a.pdf')",w.context),false);
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/a.zip')",w.context),true);
 const restarted=worker({saved:w.getSaved()});await vm.runInContext('captureReady',restarted.context);
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/a.cbz')",restarted.context),true);
 await save({enabled:true,extensions:[]});
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/a.zip')",w.context),false);
 const defaults=vm.runInContext('Array.from(DEFAULT_CAPTURE_EXTENSIONS)',w.context);
 await save({enabled:true,extensions:defaults});
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/a.pdf')",w.context),true);
 assert.equal(vm.runInContext("shouldCaptureUrl('https://example.com/a.cbz')",w.context),false);

 w.context.audioPayload={url:'https://example.com/video.mp4',outputContainer:'mp3',audioLanguage:'tr'};
 await vm.runInContext('sendToNativeHost(audioPayload,1)',w.context);
 const audio=w.calls.filter(c=>c[0]==='native').at(-1)[1];
 assert.equal(audio.transfer.audioLanguage,'tr');assert.equal(audio.transfer.container,'mp3');assert.equal(audio.transfer.connections,10);assert.equal(audio.transfer.maxHeight,1080);

 await save({enabled:true,extensions:['zip'],excludedSites:['*.example.org','https://example.com/private/*']});
 assert.equal(vm.runInContext("excludedCapture('https://cdn.example.org/a.zip')",w.context),true);
 assert.equal(vm.runInContext("excludedCapture('https://cdnXexample.org/a.zip')",w.context),false);
 assert.equal(vm.runInContext("excludedCapture('https://example.com/private/a.zip')",w.context),true);
 assert.equal(vm.runInContext("excludedCapture('https://example.com/public/a.zip')",w.context),false);
 const item={id:5,url:'https://example.com/archive.zip',filename:'/tmp/archive.zip'};
 for(const nativeOk of [true,false]){
  const x=worker({nativeOk});await x.listeners.created(item);
  const names=x.calls.map(c=>c[0]);assert.deepEqual(names,nativeOk?['pause','native','cancel','erase']:['pause','native','resume']);
 }
 const duplicate=worker({saved:{enabled:true,extensions:['zip']}});
 await duplicate.listeners.created(item);
 duplicate.calls.length=0;
 await duplicate.listeners.created({...item,id:6});
 assert.deepEqual(duplicate.calls.map(c=>c[0]),['cancel','erase']);
 assert.equal(duplicate.calls[0][1],6);
 // Removing the extension takes precedence over recent-capture deduplication.
 await new Promise(resolve=>duplicate.listeners.message({type:'popup-control',settings:{enabled:true,extensions:['pdf']},tabId:1},{url:'chrome-extension://test/popup.html'},resolve));
 duplicate.calls.length=0;
 await duplicate.listeners.created({...item,id:7});
 assert.deepEqual(duplicate.calls,[]);
 const disabled=worker({saved:{enabled:false,groups:['compressed'],custom:''}});await disabled.listeners.created(item);assert.equal(disabled.calls.length,0);
 for(const [settings,nativeOk,handled] of [[{enabled:true,extensions:['zip']},true,true],[{enabled:true,extensions:['zip']},false,true],[{enabled:true,extensions:['pdf']},true,false],[{enabled:false,extensions:['zip']},true,false]]){
  const x=worker({saved:settings,nativeOk});await vm.runInContext('captureReady',x.context);
  const result=await new Promise(resolve=>x.listeners.message({type:'capture-file-link',url:'https://example.com/archive.zip',fileName:''},{url:'https://example.com/page',tab:{id:1}},resolve));
  assert.equal(result.handled,handled);if(handled)assert.equal(result.ok,nativeOk);
  assert.deepEqual(x.calls.map(c=>c[0]),handled?['native']:[]);
  if(handled && nativeOk){
   x.calls.length=0;
   await x.listeners.created(item);
   assert.deepEqual(x.calls.map(c=>c[0]),['cancel','erase']);
  }
 }
 console.log('PASS: direct link interception sends only supported files to LDM without a browser download');
 console.log('PASS: capture groups/custom extensions persist; temporary disable; desktop acceptance before cancellation; failed delivery resumes browser');
})().catch(e=>{console.error(e);process.exitCode=1});
