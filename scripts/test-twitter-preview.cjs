const {chromium}=require('playwright');
const fs=require('node:fs'),assert=require('node:assert/strict');
(async()=>{
 const browser=await chromium.launch({executablePath:'/usr/bin/microsoft-edge-stable',headless:true,args:['--no-sandbox']});
 try {
  const page=await browser.newPage({viewport:{width:1000,height:900}});
  await page.setContent(`<style>article{margin:100px 180px 60px} [data-testid=videoComponent]{width:600px;height:200px;background:#ddd}</style>
   <article><a href="https://x.com/alice/status/123"><time>Today</time></a><div data-testid="tweetText">First video title</div><div data-testid="videoComponent"><div data-testid="videoPlayer">Video preview</div></div></article>
   <article><a href="https://x.com/bob/status/456"><time>Today</time></a><div data-testid="tweetText">Second video title</div><div data-testid="videoComponent">Video preview</div></article>
   <article><img alt="Regular photo"></article>`);
  await page.addStyleTag({content:fs.readFileSync('browser/chromium/content-style.css','utf8')});
  await page.addScriptTag({content:fs.readFileSync('browser/chromium/content-script.js','utf8').replace('\nbootstrap();','')});
  await page.evaluate(()=>{window.captures=[];triggerCaptureForCandidate=c=>captures.push({url:c.url,title:c.title});injectTwitterButtons();injectTwitterButtons();});
  assert.equal(await page.locator('video').count(),0);
  assert.equal(await page.locator('.ldm-site-btn').count(),2);
  for(let i=0;i<2;i++){
   const btn=page.locator('.ldm-site-btn').nth(i);
   assert.equal(await btn.isVisible(),true);
   const b=await btn.boundingBox(),v=await page.locator('[data-testid=videoComponent]').nth(i).boundingBox();
   assert.ok(b.y+b.height<v.y);
   await btn.click();
  }
  assert.deepEqual(await page.evaluate(()=>captures),[{url:'https://x.com/alice/status/123',title:'First video title'},{url:'https://x.com/bob/status/456',title:'Second video title'}]);
  await page.evaluate(()=>{
   const shell=document.querySelector('[data-testid=videoComponent]');
   shell.querySelector('[data-testid=videoPlayer]').appendChild(document.createElement('video'));
   injectTwitterButtons();
   shell.closest('article').querySelector('a').href='https://x.com/alice/status/789/video/1';
  });
  assert.equal(await page.locator('.ldm-site-btn').count(),2);
  await page.locator('.ldm-site-btn').first().click();
  assert.equal(await page.evaluate(()=>captures.at(-1).url),'https://x.com/alice/status/789');
  await page.addScriptTag({content:`window.testVideoAction = ${fs.readFileSync('browser/chromium/content-script.js','utf8').match(/function triggerCaptureForCandidate[\s\S]*?(?=\nfunction triggerCaptureOnce)/)[0].replace('function triggerCaptureForCandidate','function')};`});
  const outcomes=await page.evaluate(()=>{
   isYtdlpSupportedSite=()=>true;
   const results=[];
   showQualityPicker=()=>results.push('menu');
   triggerCaptureWithFormat=(c,f,container)=>results.push({f,container});
   for(const enabled of [false,true,false]){
    sendExtensionMessage=(request,done)=>done({ok:true,showVideoQualityMenu:enabled,preferences:{container:'mkv',maxHeight:720}});
    testVideoAction({url:'https://x.com/alice/status/123'},document.querySelector('.ldm-site-btn'));
   }
   return results;
  });
  assert.deepEqual(outcomes,[{f:'bv*+ba/b',container:'mkv'},'menu',{f:'bv*+ba/b',container:'mkv'}]);
  console.log('PASS: Twitter posters without playback, scoped post titles/URLs, nested player deduplication, reused timeline shells');
 } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
