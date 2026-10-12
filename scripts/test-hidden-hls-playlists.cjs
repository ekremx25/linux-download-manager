// A CDN that publishes HLS playlists as .txt under /hls/ must not let an audio-only
// rendition be classified as a video candidate.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const noop=()=>{};
const api=new Proxy(noop,{get:()=>api});
const worker={chrome:api,URL,console,self:{navigator:{userAgent:'test'}}};
vm.createContext(worker);
vm.runInContext(fs.readFileSync('browser/chromium/service-worker.js','utf8'),worker);
const run=(code)=>vm.runInContext(code,worker);

const base='https://balancehls4.playmix.uno/hls/12monkeys-s01e02.mp4/';
const audio=base+'sublist_aud1.txt';
const video=base+'sublist_2.txt';
const master=base+'master.txt';

assert.equal(run(`vidramePlaylistKind(${JSON.stringify(audio)})`),'audio');
assert.equal(run(`vidramePlaylistKind(${JSON.stringify(video)})`),'playlist');
assert.equal(run(`vidramePlaylistKind(${JSON.stringify(master)})`),'master');

assert.equal(run(`classifyMediaCandidate(${JSON.stringify(audio)},'media')`),'audio');
assert.ok(
  run(`scoreMediaCandidate(${JSON.stringify(master)},'media')`)>run(`scoreMediaCandidate(${JSON.stringify(audio)},'media')`),
  'the master playlist must outscore an audio rendition'
);

// The player requests all three; clicking download must still yield the video.
const page='https://hdfilmcehennemi.mobi/video/embed/x/';
run(`rememberMediaRequest(1,${JSON.stringify(audio)},'media',${JSON.stringify(page)});`);
run(`rememberMediaRequest(1,${JSON.stringify(video)},'media',${JSON.stringify(page)});`);
run(`rememberMediaRequest(1,${JSON.stringify(master)},'media',${JSON.stringify(page)});`);
const chosen=run(`chooseBestMediaCapturePayload(1,${JSON.stringify(audio)},${JSON.stringify(page)},'12 Maymun',${JSON.stringify(page)})`);
assert.equal(chosen.ok,true);
assert.notEqual(chosen.capture.url,audio,'an audio rendition must never be the captured video');
assert.equal(chosen.capture.url,master,'the master playlist carries both video and audio');
console.log('PASS: hidden .txt HLS playlists classify audio renditions as audio and pick the master video');