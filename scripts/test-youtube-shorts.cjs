const {chromium}=require('playwright');
const fs=require('node:fs'),assert=require('node:assert/strict');
(async()=>{
 const browser=await chromium.launch({executablePath:process.env.LDM_BROWSER||'/usr/bin/microsoft-edge-stable',headless:true,args:['--no-sandbox']});
 try {
 const page=await browser.newPage();
 await page.route('https://www.youtube.com/**',route=>route.fulfill({body:'<ytd-masthead><div id="end"><div id="buttons"></div></div></ytd-masthead><ytd-reel-video-renderer is-active><video style="width:320px;height:500px"></video></ytd-reel-video-renderer>',contentType:'text/html'}));
 await page.goto('https://www.youtube.com/shorts/first');
 await page.addScriptTag({content:fs.readFileSync('browser/chromium/content-script.js','utf8').replace('\nbootstrap();','')});
 await page.evaluate(()=>{injectYouTubeButton();window.capture=null;triggerCaptureForCandidate=c=>window.capture={url:c.url,kind:c.kind};});
 const button=page.locator('#ldm-yt-download-btn');
 assert.equal(await button.count(),1);
 await button.click();assert.equal(await page.evaluate(()=>capture.url),'https://www.youtube.com/shorts/first');
 await page.evaluate(()=>{history.pushState({},'', '/shorts/second');document.querySelector('ytd-reel-video-renderer').remove();document.body.insertAdjacentHTML('beforeend','<ytd-reel-video-renderer is-active><video style="width:320px;height:500px"></video></ytd-reel-video-renderer>');injectYouTubeButton();refreshSiteButtonPositions();});
 assert.equal(await button.count(),1);
 await button.click();assert.equal(await page.evaluate(()=>capture.url),'https://www.youtube.com/shorts/second');
 await page.evaluate(()=>{selectedAudioLanguage=async()=>null;showToast=()=>{};armCaptureTimeout=()=>{};sendExtensionMessage=msg=>window.sent=msg;});
 await page.evaluate(()=>triggerCaptureWithFormat({url:location.href,kind:'media-fallback'},'720p','mp4'));
 assert.equal(await page.evaluate(()=>sent.payload.sourcePageUrl),'https://www.youtube.com/shorts/second');
 assert.equal(await page.evaluate(()=>sent.payload.audioLanguage),null);
 console.log('PASS: Shorts button, active-player replacement, current URL, missing audio metadata');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exit(1)});
