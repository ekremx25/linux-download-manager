const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
class Element {}
const frame=(src,width=690,height=388,top=0)=>Object.assign(new Element(),{tagName:'IFRAME',getAttribute:()=>src,getBoundingClientRect:()=>({width,height,top,left:0,right:width,bottom:top+height})});
const c={Element,URL,window:{location:{href:'https://example.com/film',hostname:'example.com'},innerWidth:1280,innerHeight:800}};
vm.createContext(c);
vm.runInContext(fs.readFileSync('browser/chromium/content-script.js','utf8').replace('\nbootstrap();',''),c);
function get(f){c.frame=f;return vm.runInContext('candidateFromPlayerFrame(frame)',c);}
assert.equal(get(frame('https://hotstream.club/embed/example')).kind,'media-fallback');
assert.equal(get(frame('https://hotstream.club/embed/example')).url,null,'never download the HTML embed URL');
assert.equal(get(frame('https://video.example/player.html')).kind,'media-fallback');
assert.equal(get(frame('https://facebook.com/plugins/like.php',300,22)),null);
assert.equal(get(frame('https://youtube.com/embed/trailer',0,0)),null);
assert.equal(get(frame('https://example.com/embed/offscreen',690,388,900)),null);
assert.equal(get(frame('javascript:alert(1)')),null);
console.log('PASS: visible player frames captured; hidden trailers, widgets and non-HTTP URLs ignored');

// A trailer with a better score must not win over the explicitly chosen player.
const noop=()=>{};
const api=new Proxy(noop,{get:()=>api});
const worker={chrome:api,URL,console,self:{navigator:{userAgent:'test'}}};
vm.createContext(worker);
vm.runInContext(fs.readFileSync('browser/chromium/service-worker.js','utf8'),worker);
vm.runInContext(`
rememberMediaRequest(1,'https://trailer.example/master.m3u8','observed-manifest','https://youtube.com/embed/trailer');
`,worker);
const select=()=>vm.runInContext("chooseBestMediaCapturePayload(1,null,'https://example.com/film','Film','https://hotstream.club/embed/main')",worker);
assert.equal(select().ok,false,'unrelated trailer must not trigger a page-extractor fallback');
vm.runInContext("rememberMediaRequest(1,'https://media.example/stream','observed-manifest','https://hotstream.club/embed/main')",worker);
let chosen=select();
assert.equal(chosen.capture.url,'https://media.example/stream');
assert.equal(chosen.capture.streamManifest,true);
assert.equal(chosen.capture.sourcePageUrl,'https://hotstream.club/embed/main');
assert.deepEqual(Array.from(chosen.capture.fallbackUrls),[],'trailer must not become a fallback');
assert.equal(vm.runInContext("candidateBelongsToPlayer({referrerUrl:'https://hotstream.club/embed/other'},'https://hotstream.club/embed/main')",worker),false);
console.log('PASS: selected player stream wins; trailers and unrelated fallback URLs excluded');

// Player responses with misleading MIME types can still identify an HLS manifest.
const published=[];
class XHR {addEventListener(_,fn){this.loaded=fn;} open(){}}
const observer={URL,TextDecoder,XMLHttpRequest:XHR,location:{href:'https://hotstream.club/embed/main',origin:'https://hotstream.club'},window:{addEventListener(){},postMessage:m=>published.push(m)}};
vm.createContext(observer);
vm.runInContext(fs.readFileSync('browser/chromium/player-manifest-observer.js','utf8'),observer);
const xhr=new XHR();xhr.open('GET','/stream');xhr.status=200;xhr.responseURL='https://media.example/stream';xhr.responseText='#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nsegment.ts';xhr.loaded();
assert.equal(published[0].url,'https://media.example/stream');
xhr.responseText='<html>not a playlist</html>';xhr.loaded();assert.equal(published.length,1);
const manifest=JSON.parse(fs.readFileSync('browser/chromium/manifest.json','utf8'));
assert.ok(manifest.content_scripts.some(s=>s.world==='MAIN'&&s.matches.includes('https://*.hotstream.club/*')));
console.log('PASS: player manifest response recognized without extra network requests');

// Regression: the selected Dizibox frame wraps a second, cross-origin player.
vm.runInContext(`
rememberPlayerFrames(2,0,'https://www.dizibox.live/episode',[
  'https://www.dizibox.live/player/king/king.php?v=main', 'https://ad.example/embed/ad']);
rememberPlayerFrames(2,42,'https://www.dizibox.live/player/king/king.php?v=main',[
  'https://dbx.molystream.org/embed/main']);
rememberPlayerFrames(2,43,'https://dbx.molystream.org/embed/main',[]);
rememberMediaRequest(2,'https://dbx.molystream.org/opaque','observed-manifest','https://dbx.molystream.org/embed/main');
rememberMediaRequest(2,'https://ad.example/master.m3u8','observed-manifest','https://ad.example/embed/ad');
`,worker);
chosen=vm.runInContext("chooseBestMediaCapturePayload(2,null,'https://www.dizibox.live/episode','Episode','https://www.dizibox.live/player/king/king.php?v=main')",worker);
assert.equal(chosen.ok,true);
assert.equal(chosen.capture.url,'https://dbx.molystream.org/opaque');
assert.equal(chosen.capture.sourcePageUrl,'https://dbx.molystream.org/embed/main');
assert.equal(chosen.capture.streamManifest,true);
assert.deepEqual(Array.from(chosen.capture.fallbackUrls),[]);
// A navigated frame must replace the old child relationship, not accumulate it.
vm.runInContext("rememberPlayerFrames(2,42,'https://www.dizibox.live/player/king/king.php?v=main',[])",worker);
assert.equal(vm.runInContext("candidateBelongsToPlayer({referrerUrl:'https://dbx.molystream.org/embed/main'},'https://www.dizibox.live/player/king/king.php?v=main',2)",worker),false);
assert.equal(vm.runInContext("candidateBelongsToPlayer({referrerUrl:'https://dbx.molystream.org/embed/main'},'https://www.dizibox.live/player/king/king.php?v=main',99)",worker),false);
// Cycles in reported embeds cannot hang the extension.
vm.runInContext("rememberPlayerFrames(2,42,'https://www.dizibox.live/player/king/king.php?v=main',['https://www.dizibox.live/player/king/king.php?v=main'])",worker);
assert.equal(vm.runInContext("candidateBelongsToPlayer({referrerUrl:'https://unrelated.example/'},'https://www.dizibox.live/player/king/king.php?v=main',2)",worker),false);
console.log('PASS: nested player captured, sibling ads excluded, navigation replaces ancestry, cycles bounded');
