const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const assert = require('node:assert/strict');
const messages = [];
class XHR {
  addEventListener(_, cb){this.loaded=cb;}
  open(){return 'original-open';}
}
const window = {addEventListener(){},postMessage:m=>messages.push(m)};
const context = vm.createContext({window,XMLHttpRequest:XHR,URL,TextDecoder,
 location:{href:'https://player.molystream.org/embed',origin:'https://player.molystream.org'}});
vm.runInContext(fs.readFileSync(path.join(__dirname,'../chromium/player-manifest-observer.js'),'utf8'),context);
const xhr=new XHR();
assert.equal(xhr.open('GET','/opaque'),'original-open');
xhr.status=200;xhr.responseURL='https://cdn.example/opaque';xhr.responseType='';
xhr.responseText='<html>not a playlist</html>';xhr.loaded();assert.equal(messages.length,0);
xhr.responseText='#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nsegment.png\n';
xhr.loaded();assert.equal(messages[0].url,xhr.responseURL);
xhr.status=403;xhr.loaded();assert.equal(messages.length,1);
console.log('Playlist body identification checks passed.');
