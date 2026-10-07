// Small dependency-free regressions for live status and mutation-storm coalescing.
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const context = {window:{location:{href:'https://www.youtube.com/watch?v=test',hostname:'www.youtube.com'}}};
let pending = [];
context.window.setTimeout = fn => { pending.push(fn); return pending.length; };
context.document = {hidden:false};
vm.createContext(context);
const content = fs.readFileSync('browser/chromium/content-script.js','utf8').replace('\nbootstrap();','');
vm.runInContext(content, context);
vm.runInContext('reportObservedMediaCandidates=()=>{}; refreshRecentMediaCandidates=()=>{};',context);
for(let i=0;i<1000;i++) vm.runInContext('scheduleMediaRefresh()',context);
assert.equal(pending.length,1,'DOM storm must schedule a single scan');
pending.shift()();
vm.runInContext('scheduleMediaRefresh()',context);
assert.equal(pending.length,1,'later changes must still schedule another scan');
// Background tabs neither schedule scans nor run callbacks queued while visible.
pending.shift()();
let scans = 0;
context.recordScan = () => { scans++; };
vm.runInContext('reportObservedMediaCandidates=recordScan; refreshRecentMediaCandidates=recordScan; refreshMediaOverlay=recordScan; countCandidates=recordScan; sendExtensionMessage=()=>{};', context);
for(let i=0;i<1000;i++) vm.runInContext('scheduleMediaRefresh(); scheduleMediaOverlayRefresh(); scheduleCandidateReport();',context);
assert.equal(pending.length,3,'each scan type has at most one pending callback');
context.document.hidden=true;
while(pending.length) pending.shift()();
assert.equal(scans,0,'queued scans must stop when the tab becomes hidden');
vm.runInContext('scheduleMediaRefresh(); scheduleMediaOverlayRefresh(); scheduleCandidateReport();',context);
assert.equal(pending.length,0,'hidden tabs must not schedule scanning work');
context.document.hidden=false;
vm.runInContext('scheduleMediaRefresh();',context);
pending.shift()();
assert.equal(scans,2,'visible tab must resume candidate discovery');
console.log('PASS: hidden-tab scans stopped, visible-tab discovery resumed');
const ui = fs.readFileSync('ui/main.js','utf8').replace(/^import .*;\n/,'');
const c = {document:{addEventListener(){}},formatSpeed:n=>`${n}/s`,formatTime:n=>`${n}s`};
vm.createContext(c);vm.runInContext(ui,c);
assert.equal(vm.runInContext("speedFor({id:1,status:'queued'},{status:'in_progress',speedBytesPerSecond:2048})",c),'2048/s');
assert.equal(vm.runInContext("etaFor({id:1,status:'queued'},{status:'in_progress',etaSeconds:12})",c),'12s');
assert.equal(vm.runInContext("etaFor({id:2,status:'in_progress'},{totalBytes:null,speedBytesPerSecond:2048})",c),'—');
console.log('PASS: coalesced scans, live speed/ETA before DB refresh, unknown duration');

context.ownNode={nodeType:1,closest:()=>({})};
context.pageNode={nodeType:1,closest:()=>null};
assert.equal(vm.runInContext("isExtensionMutation({type:'childList',target:ownNode,addedNodes:[pageNode]})",context),true);
assert.equal(vm.runInContext("isExtensionMutation({type:'childList',target:pageNode,addedNodes:[ownNode]})",context),true);
assert.equal(vm.runInContext("isExtensionMutation({type:'childList',target:pageNode,addedNodes:[pageNode]})",context),false);
console.log('PASS: extension overlay updates do not retrigger media scans');

// Bulk actions affect only eligible selections; a completed file is never resumed.
(async()=>{
  const elements=new Map();
  c.document.getElementById=id=>{if(!elements.has(id))elements.set(id,{setAttribute(){},dataset:{}});return elements.get(id);};
  c.isTauri=false;
  const calls=[];c.invoke=async(command,args)=>calls.push([command,args.id]);
  c.setNotice=(element,text,type)=>{element.textContent=text;element.type=type;};
  c.categoryFor=d=>d.category||'other';c.statusLabel=s=>s;
  vm.runInContext("state.downloads=[{id:1,status:'in_progress'},{id:2,status:'paused'},{id:3,status:'completed'},{id:4,status:'failed'}];state.selected=new Set([2,3]);",c);
  await vm.runInContext("batch('resume_download',true)",c);
  assert.deepEqual(calls,[['resume_download',2]]);
  calls.length=0;
  await vm.runInContext("batch('pause_download')",c);
  assert.deepEqual(calls,[['pause_download',1]]);
  vm.runInContext("state.filter='failed';",c);
  assert.equal(vm.runInContext('filtered().length',c),1);
  console.log('PASS: selected-only resume, active-only pause, failure filter');
})().catch(error=>{console.error(error);process.exitCode=1;});
