const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const assert = require('node:assert/strict');
let attempts = 0;
const context = vm.createContext({URL, console,
 window: {location: {href: 'https://example.test/player'}, clearInterval(){}, clearTimeout(){}},
 chrome: {runtime:{id:'test',sendMessage(){attempts++;throw new Error('Extension context invalidated.');}}}
});
const source = fs.readFileSync(path.join(__dirname,'../chromium/content-script.js'),'utf8');
vm.runInContext(source.replace('\nbootstrap();','\n'),context);
context.showToast = () => {};
const link = {hasAttribute: () => false};
for(const invalid of [undefined, '', 'http://[broken', 'javascript:alert(1)', 'blob:https://example.test/uuid']) {
 assert.equal(context.isMediaCandidate(invalid),false);
 assert.equal(context.isDownloadableLink(link,invalid),false);
}
assert.equal(context.isMediaCandidate('/movie.mp4'),true);
assert.equal(context.isDownloadableLink(link,'/file.zip'),true);
assert.doesNotThrow(()=>context.sendExtensionMessage({type:'test'}));
context.sendExtensionMessage({type:'test-again'});
assert.equal(attempts,1);
console.log('Malformed URL and invalidated extension checks passed.');
