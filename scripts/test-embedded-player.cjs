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
vm.runInContext("rememberMediaRequest(31,'https://dbx.molystream.org/embed/sheila/example','media','https://dbx.molystream.org/embed/sheila/example')",worker);
let molystream=vm.runInContext("chooseBestMediaCapturePayload(31,null,'https://yabancidizi.news/dizi/lucky-2026/sezon-1/bolum-1','Lucky','https://dbx.molystream.org/embed/sheila/example')",worker);
assert.equal(molystream.capture.url,'https://dbx.molystream.org/embed/sheila/example');
assert.equal(molystream.capture.forceYtdlp,true,'embed page must use extractor instead of direct HTTP');
assert.equal(vm.runInContext("mediaOptionsForTab(31,'https://dbx.molystream.org/embed/sheila/example').length",worker),0,'embed HTML must not appear as a video format');
vm.runInContext("rememberMediaRequest(31,'https://media.example/lucky/master.m3u8','observed-manifest','https://dbx.molystream.org/embed/sheila/example')",worker);
molystream=vm.runInContext("chooseBestMediaCapturePayload(31,null,'https://yabancidizi.news/dizi/lucky-2026/sezon-1/bolum-1','Lucky','https://dbx.molystream.org/embed/sheila/example')",worker);
assert.equal(molystream.capture.url,'https://media.example/lucky/master.m3u8','observed stream wins over the embed page');
console.log('PASS: Molystream embed uses yt-dlp only until its real stream is captured');
vm.runInContext(`
rememberMediaRequest(12,'https://cdn.example/video_720p.m3u8','observed-manifest','https://player.example/embed/main');
rememberMediaRequest(12,'https://cdn.example/video_1080p.m3u8','observed-manifest','https://player.example/embed/main');
rememberMediaRequest(12,'https://cdn.example/audio.m3u8','audio','https://player.example/embed/main');
rememberMediaRequest(12,'https://ads.example/trailer_720p.m3u8','observed-manifest','https://ads.example/embed/ad');
`,worker);
const options=vm.runInContext("mediaOptionsForTab(12,'https://player.example/embed/main')",worker);
assert.equal(options.length,2);
assert.ok(options.some(option=>option.label.includes('1080p')));
assert.ok(options.every(option=>option.url.startsWith('https://cdn.example/')));
console.log('PASS: player stream chooser lists resolutions without audio-only or ad streams');
const refreshedStream=vm.runInContext(`replacementForMediaJob(
 {url:'https://cdn.example/videos/42/master.m3u8?token=old',referrerUrl:'https://player.example/embed/main'},
 [{url:'https://ads.example/videos/42/master.m3u8?token=new',streamKind:'playlist',referrerUrl:'https://ads.example/embed/ad'},
  {url:'https://cdn.example/videos/43/master.m3u8?token=new',streamKind:'playlist',referrerUrl:'https://player.example/embed/main'},
  {url:'https://cdn.example/videos/42/master.m3u8?token=new',streamKind:'playlist',referrerUrl:'https://player.example/embed/main'}])`,worker);
assert.equal(refreshedStream.url,'https://cdn.example/videos/42/master.m3u8?token=new');
assert.equal(vm.runInContext(`replacementForMediaJob(
 {url:'https://cdn.example/videos/42/master.m3u8?token=old',referrerUrl:'https://player.example/embed/main'},
 [{url:'https://cdn.example/videos/43/master.m3u8?token=new',streamKind:'playlist',referrerUrl:'https://player.example/embed/main'}])`,worker),null);
console.log('PASS: expired video recovery selects only a new link for the same player and media path');

vm.runInContext("rememberMediaRequest(15,'https://cdn.example/master.m3u8','observed-manifest','https://player.example/embed/main',{'x-test-header':'observed'})",worker);
const direct=vm.runInContext("buildDirectCapturePayload(15,{url:'https://cdn.example/master.m3u8',sourcePageUrl:'https://outer.example/film'},null)",worker);
assert.equal(direct.sourcePageUrl,'https://player.example/embed/main');
assert.equal(direct.httpHeaders.referer,'https://player.example/embed/main');
assert.equal(direct.httpHeaders.origin,'https://player.example');
assert.equal(direct.httpHeaders['x-test-header'],'observed');
assert.equal(direct.streamManifest,true);
console.log('PASS: direct capture carries observed player headers and referrer');

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

// Vidrame serves master/video/audio manifests as .txt. A recently fetched audio
// playlist must never replace the master or become a video fallback.
vm.runInContext(`
rememberMediaRequest(9,'https://vidrame.live/vr/get/70332b6d/master.txt','observed-manifest','https://vidrame.pro/');
rememberMediaRequest(9,'https://vidrame.live/vr/get/70332b6d/1080.txt','observed-manifest','https://vidrame.pro/');
rememberMediaRequest(9,'https://vidrame.live/vr/get/70332b6d/audio-tur-1.txt','observed-manifest','https://vidrame.pro/');
`,worker);
chosen=vm.runInContext("chooseBestMediaCapturePayload(9,null,'https://vidrame.pro/','Episode')",worker);
assert.equal(chosen.capture.url,'https://vidrame.live/vr/get/70332b6d/master.txt');
assert.equal(chosen.capture.audioUrl,null);
assert.equal(chosen.capture.streamManifest,true);
assert.deepEqual(Array.from(chosen.capture.fallbackUrls),[]);
vm.runInContext("rememberMediaRequest(10,'https://vidrame.live/vr/get/70332b6d/audio-tur-1.txt','observed-manifest','https://vidrame.pro/')",worker);
assert.equal(vm.runInContext("chooseBestMediaCapturePayload(10,null,'https://vidrame.pro/','Episode','https://vidrame.pro/')",worker).ok,false);
console.log('PASS: Vidrame master wins over audio-only and video-only playlists; audio-only capture rejected');

for(const url of ['https://www.instagram.com/reel/example/','https://www.facebook.com/reel/123456','https://www.facebook.com/watch/?v=123456']){
 worker.socialUrlFixture=url;
 const result=vm.runInContext("chooseBestMediaCapturePayload(99,null,socialUrlFixture,'Social video')",worker);
 assert.equal(result.capture.url,url);assert.equal(result.capture.forceYtdlp,true);
 c.location={href:url,hostname:new URL(url).hostname};c.document={title:'Social video'};
 c.socialVideo={closest:()=>null,currentSrc:'blob:sample'};
 assert.equal(vm.runInContext('socialVideoCandidate(socialVideo)',c).url,url.includes('facebook')?'https://www.facebook.com/watch/?v=123456':url);
}
c.location={href:'https://www.instagram.com/',hostname:'www.instagram.com'};
assert.equal(vm.runInContext('socialVideoCandidate(socialVideo)',c),null,'ambiguous feed blob must not select another post');
console.log('PASS: Instagram/Facebook post and reel capture uses the selected permalink; ambiguous feed blob rejected');

c.location={href:'https://www.instagram.com/reel/abc/',hostname:'www.instagram.com',pathname:'/reel/abc/'};
c.document={title:'Instagram',querySelector:s=>s.includes('og:description')?{content:'20 likes, author on October 12: "Beautiful autumn in Istanbul".'}:null};
assert.equal(vm.runInContext("instagramVideoTitle(null,null,'https://www.instagram.com/reel/abc/')",c),'Beautiful autumn in Istanbul');
c.captionPost={querySelector:()=>({textContent:'My own video caption'})};
assert.equal(vm.runInContext("instagramVideoTitle(null,captionPost,'https://www.instagram.com/reel/abc/')",c),'My own video caption');
assert.equal(vm.runInContext("instagramVideoTitle(null,null,'https://www.instagram.com/reel/other/')",c),'Instagram video','do not borrow another reel metadata');
c.document.querySelector=()=>null;
assert.equal(vm.runInContext("instagramVideoTitle(null,null,'https://www.instagram.com/reel/abc/')",c),'Instagram video');
console.log('PASS: Instagram filenames use post caption, scoped metadata, or readable fallback');

assert.equal(vm.runInContext("cleanInstagramCaption('İstanbul’da güzel bir gün 🌞 #reels #keşfet @someone https://example.com')",c),'İstanbul da güzel bir gün');
assert.equal(vm.runInContext("cleanInstagramCaption('#reels @user 🌞')",c),'');
assert.ok(vm.runInContext("cleanInstagramCaption('word '.repeat(100)).length",c)<=80);
console.log('PASS: short readable captions, no hashtags/mentions/URLs or random-code fallback');

// Successful/sending notifications return before touching DOM on every site.
c.window.location.hostname='www.youtube.com';
vm.runInContext("showToast('Download queued in Linux Download Manager.','success');showToast('Sending download to Linux Download Manager...','info')",c);
console.log('PASS: sending and success banners suppressed across sites');

for(const url of ['https://www.facebook.com/reels/123456/','https://www.facebook.com/video.php?v=123456','https://www.facebook.com/user/videos/123456/']){
 c.location={href:url,hostname:'www.facebook.com',pathname:new URL(url).pathname};
 assert.equal(vm.runInContext('socialVideoCandidate(socialVideo)',c).url,'https://www.facebook.com/watch/?v=123456');
}
c.location={href:'https://www.facebook.com/',hostname:'www.facebook.com',pathname:'/'};
c.socialVideo.parentElement={parentElement:null,querySelector:()=>null,querySelectorAll:s=>s==='video'?[c.socialVideo]:[{href:'https://www.facebook.com/reels/123456/'}]};
assert.equal(vm.runInContext('socialVideoCandidate(socialVideo)',c).url,'https://www.facebook.com/watch/?v=123456');
c.socialVideo.parentElement.querySelectorAll=s=>s==='video'?[c.socialVideo,{}]:[{href:'https://www.facebook.com/reels/999/'}];
assert.equal(vm.runInContext('socialVideoCandidate(socialVideo)',c),null);
console.log('PASS: Facebook reels, video.php and single-player containers; multi-video feed is not guessed');

c.socialVideo.parentElement={parentElement:null,querySelectorAll:s=>s==='video'?[c.socialVideo]:[{href:'https://www.facebook.com/oyunfor/posts/pfbidExample/'}],querySelector:()=>({textContent:'Dust2’de neler oluyor? 😅 #oyunfor #dust2 #counterstrike'})};
let fb=vm.runInContext('socialVideoCandidate(socialVideo)',c);
assert.equal(fb.url,'https://www.facebook.com/oyunfor/posts/pfbidExample/');
assert.equal(fb.title,'Dust2 de neler oluyor');
worker.socialUrlFixture=fb.url;
assert.equal(vm.runInContext("chooseBestMediaCapturePayload(99,null,socialUrlFixture,'Dust2')",worker).capture.forceYtdlp,true);
console.log('PASS: Facebook feed post links and scoped captions');
vm.runInContext(`
{
 const base='https://s230.rapidrame.com/hls2/01/00036/example.urlset/';
 const audio={url:base+'index-f1-a1.m3u8',streamKind:classifyMediaCandidate(base+'index-f1-a1.m3u8','observed-manifest')};
 const video={url:base+'index-f2-v1-a1.m3u8',streamKind:classifyMediaCandidate(base+'index-f2-v1-a1.m3u8','observed-manifest')};
 const master={url:base+'master.m3u8',streamKind:classifyMediaCandidate(base+'master.m3u8','observed-manifest')};
 if(audio.streamKind!=='audio'||selectVideoCandidate([audio,video,master],audio)!==master||selectVideoCandidate([audio],audio)!==null)throw Error('Rapidrame selected audio as video');
}
`,worker);
console.log('PASS: Rapidrame master selected over audio-only rendition');

assert.ok(manifest.content_scripts.some(s=>s.world==='MAIN'&&s.matches.includes('https://*.rapidvid.org/*')));
vm.runInContext(`rememberMediaRequest(9,'https://cdn.example/opaque','observed-manifest','https://rapidvid.org/vx/example')`,worker);
const rapid=vm.runInContext("chooseBestMediaCapturePayload(9,null,'https://example.com/film','Film','https://rapidvid.org/vx/example')",worker);
assert.equal(rapid.capture.url,'https://cdn.example/opaque');
assert.equal(rapid.capture.streamManifest,true);
assert.equal(rapid.capture.sourcePageUrl,'https://rapidvid.org/vx/example');
console.log('PASS: Rapidvid observer enabled and player manifest routed to capture');

const rapidFrame=get(frame('https://rapidvid.org/vx/v1xd0df7f84'));
assert.equal(rapidFrame.kind,'media-fallback');
assert.equal(rapidFrame.url,null);
assert.equal(rapidFrame.playerPageUrl,'https://rapidvid.org/vx/v1xd0df7f84');
assert.equal(get(frame('https://rapidvid.org/vx/id',0,0)),null);
assert.equal(get(frame('https://rapidvid.org.evil.example/vx/id')),null);
assert.equal(get(frame('https://example.com/vx/id')),null);
assert.equal(get(frame('https://rapidvid.org/account')),null);
console.log('PASS: visible Rapidvid /vx/ frames get a download target; unrelated frames do not');

// Lazy embeds are present before the user starts playback.
const lazyFrame=frame(null,1010,586,100);
lazyFrame.getAttribute=name=>name==='data-src'?'https://rapidvid.org/vx/example':'about:blank';
assert.equal(get(lazyFrame).playerPageUrl,'https://rapidvid.org/vx/example');
// Keep a visible control when the player is partially above the viewport.
c.document={fullscreenElement:null};
c.positionButton={id:'',dataset:{},style:{setProperty(){}},hidden:true};
for(const [top,hidden] of [[100,false],[0,false],[-100,false],[-600,true],[900,true]]){
 c.positionFrame=frame('https://rapidvid.org/vx/example',1010,586,top);
 vm.runInContext('placeAbovePlayer(positionButton,positionFrame)',c);
 assert.equal(c.positionButton.hidden,hidden,`player top=${top}`);
}
console.log('PASS: lazy player recognized; scrolled player retains control; offscreen players hidden');

// Recognized embeds must render independently of generic media-container scans.
c.window.location={href:'https://www.fullhdfilmizlesene.now/film/example/',hostname:'www.fullhdfilmizlesene.now'};
c.location=c.window.location;
const attached=[];
const listeners=new Map();
let currentEmbedUrl='https://rapidvid.org/vx/first';
const visibleEmbed=frame(currentEmbedUrl,1010,586,170);
visibleEmbed.isConnected=true;
visibleEmbed.getAttribute=()=>currentEmbedUrl;
c.document={fullscreenElement:null,
 querySelectorAll(selector){
  if(selector==='iframe[src], iframe[data-src]')return [visibleEmbed];
  throw new Error('Generic scan must not run for an identified player');
 },
 documentElement:{appendChild(button){button.isConnected=true;if(!attached.includes(button))attached.push(button);}},
 createElement(){return {id:'',dataset:{},hidden:false,isConnected:false,
  classList:{add(){}},style:{setProperty(){}},removeAttribute(){},setAttribute(){},
  addEventListener(type,fn){listeners.set(type,fn);},remove(){this.isConnected=false;}
 };}
};
vm.runInContext('mediaOverlayRoot={replaceChildren(){}}; refreshMediaOverlay()',c);
assert.equal(attached.length,1);
assert.equal(attached[0].hidden,false);
assert.equal(attached[0].textContent,'↓ Download video  ·  LDM');
vm.runInContext('refreshMediaOverlay()',c);
assert.equal(attached.length,1,'refresh must not duplicate the button');
currentEmbedUrl='https://rapidvid.org/vx/second';
vm.runInContext('triggerCaptureOnce=(candidate)=>{globalThis.clickedPlayer=candidate.playerPageUrl;}',c);
listeners.get('click')({preventDefault(){},stopPropagation(){}});
assert.equal(c.clickedPlayer,currentEmbedUrl,'click must use the current player, not stale iframe URL');
attached[0].isConnected=false;
vm.runInContext('refreshMediaOverlay()',c);
assert.equal(attached[0].isConnected,true,'a removed button must be reattached');
const adFrame=frame('https://ads.example/embed/banner',320,180,80);
adFrame.isConnected=true;
c.adFrame=adFrame;c.visibleEmbed=visibleEmbed;
c.document.querySelectorAll=selector=>{
 if(selector==='iframe[src], iframe[data-src]')return [adFrame,visibleEmbed];
 throw new Error('Generic scan must not run for an identified player');
};
vm.runInContext('refreshMediaOverlay()',c);
assert.equal(attached.filter(button=>button.isConnected).length,1,'ad iframe must not get a second button');
assert.equal(vm.runInContext('siteDownloadButtons.has(visibleEmbed)',c),true);
assert.equal(vm.runInContext('siteDownloadButtons.has(adFrame)',c),false);
vm.runInContext('hoverButton={hidden:false};activeCandidate={element:visibleEmbed,kind:"media-fallback"};repositionHoverButton()',c);
assert.equal(vm.runInContext('hoverButton.hidden',c),true,'hover button must stay hidden beside the player button');
console.log('PASS: embed button renders without generic scanning, persists without duplicates, and captures current player');

// Generic iframe paths become controllable only after a video stream is
// attributed to that frame. Unrelated ad and audio streams must not qualify.
vm.runInContext(`
rememberPlayerFrames(77,0,'https://movies.example/watch',['https://cdn.player.example/v/opaque','https://ads.example/view']);
rememberPlayerFrames(77,1,'https://cdn.player.example/v/opaque',[]);
rememberPlayerFrames(77,2,'https://ads.example/view',[]);
rememberMediaRequest(77,'https://stream.example/main.m3u8','observed-manifest','https://cdn.player.example/v/opaque');
rememberMediaRequest(77,'https://ads.example/sound.mp3','audio','https://ads.example/view');
`,worker);
const detected=vm.runInContext('detectedPlayerPagesForTab(77)',worker);
assert.equal(detected.includes('https://cdn.player.example/v/opaque'),true);
assert.equal(detected.includes('https://ads.example/view'),false);
vm.runInContext("detectedPlayerPages=new Set(['https://cdn.player.example/v/opaque'])",c);
assert.equal(get(frame('https://cdn.player.example/v/opaque',690,388,100)).kind,'media-fallback');
assert.equal(get(frame('https://ads.example/view',690,388,100)),null);
vm.runInContext("rememberPlayerFrames(77,1,'https://cdn.player.example/v/changed',[])",worker);
assert.equal(vm.runInContext('detectedPlayerPagesForTab(77)',worker).includes('https://cdn.player.example/v/changed'),false);
console.log('PASS: generic player frame recognized from its video stream; ad and stale frame excluded');

// The background response must trigger a fresh overlay scan after playback,
// without requiring a site-specific player URL or page refresh.
let overlayScheduled=0;
c.chrome={runtime:{lastError:null}};
c.window.top=c.window;
c.scheduleMediaOverlayRefresh=()=>{overlayScheduled++;};
c.sendExtensionMessage=(_message,callback)=>callback({ok:true,
 candidates:[{url:'https://stream.example/main.m3u8'}],
 detectedPlayerPages:['https://cdn.player.example/v/opaque']});
vm.runInContext('detectedPlayerPages.clear(); refreshRecentMediaCandidates()',c);
assert.equal(vm.runInContext("detectedPlayerPages.has('https://cdn.player.example/v/opaque')",c),true);
assert.equal(overlayScheduled,1);
console.log('PASS: network video detection refreshes generic player overlay after playback');

// A withdrawn signed playlist may have a working mirror for the same asset.
// Keep a different video and a different page out of the fallback list.
vm.runInContext(`
rememberMediaRequest(88,'https://mirror-b.example/hls/video-42/master.m3u8?token=old','observed-manifest','https://site.example/watch/42');
rememberMediaRequest(88,'https://mirror-a.example/hls/video-42/master.m3u8?token=new','observed-manifest','https://site.example/watch/42');
rememberMediaRequest(88,'https://mirror-c.example/hls/trailer-99/master.m3u8','observed-manifest','https://site.example/watch/42');
rememberMediaRequest(88,'https://mirror-d.example/hls/video-42/master.m3u8','observed-manifest','https://site.example/watch/other');
`,worker);
const mirrorChoice=vm.runInContext("chooseBestMediaCapturePayload(88,'https://mirror-a.example/hls/video-42/master.m3u8?token=new','https://site.example/watch/42','Video')",worker);
assert.equal(mirrorChoice.ok,true);
assert.deepEqual(Array.from(mirrorChoice.capture.fallbackUrls),['https://mirror-b.example/hls/video-42/master.m3u8?token=old']);
console.log('PASS: alternate signed playlist retried only for the same video and page');

// An observed playlist can receive 410 after its request was recorded. The
// rejected address must not outrank a successful stream from the same page.
vm.runInContext(`
rememberMediaRequest(89,'https://good-cdn.example/hls/video-7/master.m3u8','observed-manifest','https://site.example/watch/7');
rememberMediaRequest(89,'https://bad-cdn.example/hls/video-7/master.m3u8','observed-manifest','https://site.example/watch/7');
discardRejectedMediaRequest(89,'https://bad-cdn.example/hls/video-7/master.m3u8');
`,worker);
assert.equal(vm.runInContext('getMediaCandidatesForTab(89).length',worker),1);
const recovered=vm.runInContext("chooseBestMediaCapturePayload(89,'https://bad-cdn.example/hls/video-7/master.m3u8','https://site.example/watch/7','Video')",worker);
assert.equal(recovered.capture.url,'https://good-cdn.example/hls/video-7/master.m3u8');
console.log('PASS: rejected 410 playlist discarded and working stream selected');
