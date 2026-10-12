const {chromium}=require('playwright'),fs=require('node:fs'),assert=require('node:assert/strict');
(async()=>{
 const browser=await chromium.launch({executablePath:'/usr/bin/microsoft-edge-stable',headless:true,args:['--no-sandbox']});
 const page=await browser.newPage();let fileRequests=0;
 await page.route('https://files.test/**',async route=>{const file=route.request().url().endsWith('.zip');if(file)fileRequests++;await route.fulfill({contentType:'text/html',body:file?'Browser handled file':'<a href="/archive.zip">Download</a>'});});
 await page.goto('https://files.test/page');
 await page.addScriptTag({content:fs.readFileSync('browser/chromium/content-script.js','utf8').replace('\nbootstrap();','')});
 await page.evaluate(()=>{window.testHandled=true;window.chrome={runtime:{}};sendExtensionMessage=(message,cb)=>cb({handled:window.testHandled,ok:true});document.addEventListener('click',handleFileLinkClick,true);});
 await page.locator('a').click();await page.waitForTimeout(150);assert.equal(fileRequests,0);
 await page.evaluate(()=>window.testHandled=false);
 await page.locator('a').click();await page.waitForURL('**/archive.zip');assert.equal(fileRequests,1);
 await browser.close();console.log('PASS: supported link makes zero file requests; removed type uses browser navigation');
})().catch(e=>{console.error(e);process.exit(1)});
