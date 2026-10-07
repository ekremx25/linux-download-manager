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
