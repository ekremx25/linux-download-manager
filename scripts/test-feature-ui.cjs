// Optional browser regression: serve the repository on 127.0.0.1:18763 first.
// Requires Playwright; LDM_BROWSER can select a Chromium-based browser executable.
const {chromium}=require('playwright');
const assert=require('node:assert/strict');
(async()=>{
 const browser=await chromium.launch({executablePath:process.env.LDM_BROWSER||'/usr/bin/microsoft-edge-stable',headless:true,args:['--no-sandbox']});
 const page=await browser.newPage({viewport:{width:1200,height:850}});const errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{
  window.startOnLogin=false;window.speedLimit=0;window.testPalette={'bg':'rgb(240, 235, 225)','surface':'rgb(255, 250, 240)','text':'rgb(40, 35, 30)','accent':'rgb(30, 130, 90)'};window.captureRules={enabled:true,extensions:['zip','mp4'],excludedSites:[]};window.calls=[];window.handlers={};window.featureSettings={transfer:{connections:8,maxHeight:1080,container:'mp4'},showDetailOnStart:true,autoCloseDetail:false};
  window.records=[{id:1,fileName:'Zebra.iso',category:'iso',status:'in_progress',totalBytes:104857600,downloadedBytes:26214400},{id:2,fileName:'Album.mp3',category:'music',status:'paused',totalBytes:2097152,downloadedBytes:1024}];
  window.__TAURI__={core:{invoke:async(cmd,args={})=>{calls.push({cmd,args});switch(cmd){case 'system_theme':return window.testPalette;case 'capture_settings':if(args.settings)captureRules=args.settings;return {ok:true,settings:captureRules,defaults:['zip','mp4','pdf']};case 'app_settings':return {startOnLogin:window.startOnLogin,defaultDownloadDir:'/Downloads',maxConcurrentDownloads:3,defaultBandwidthLimitKbps:window.speedLimit,desktopNotificationsEnabled:true,features:featureSettings};case 'update_app_settings':if(args.startOnLogin!==undefined)window.startOnLogin=args.startOnLogin;if(args.defaultBandwidthLimitKbps!==undefined)window.speedLimit=args.defaultBandwidthLimitKbps;if(args.features)featureSettings=args.features;return {};case 'list_downloads':return records;case 'system_status':return {};case 'download_detail':return {mediaTransfer:window.mediaProgress||null,download:{...records[0],url:'https://example.com/file.iso',savePath:'/Downloads/LDM/ISO/Zebra.iso'},resumeSupported:true,segments:Array.from({length:8},(_,index)=>({index,status:'receiving',start:index*13107200,totalBytes:13107200,downloadedBytes:(index+1)*1048576}))};case 'start_download':if(args.url.includes('fail'))throw 'Fixture rejection';return {id:3};default:return {};}}},event:{listen:async(name,fn)=>{handlers[name]=fn;return ()=>{};}}};
 });
 await page.goto('http://127.0.0.1:18763/ui/index.html');await page.locator('.download-row.item').first().waitFor();
 await page.locator('[data-sort="fileName"]').click();assert.equal(await page.locator('.download-row.item .file-name').first().textContent(),'Album.mp3');
 await page.locator('[data-sort="fileName"]').click();assert.equal(await page.locator('.download-row.item .file-name').first().textContent(),'Zebra.iso');
 await page.locator('.download-row.item[data-id="1"]').click({button:'right'});
 await page.locator('[data-menu-action="cancel_download"]').click();
 assert.equal(await page.evaluate(()=>calls.filter(c=>c.cmd==='cancel_download'&&c.args.id===1).length),1);
 await page.evaluate(()=>{records[0].status='cancelled';});
 await page.waitForFunction(()=>document.querySelector('.download-row.item[data-id="1"]').textContent.includes('Cancelled'));
 await page.locator('.download-row.item[data-id="1"]').click({button:'right'});
 assert.equal(await page.locator('[data-menu-action="cancel_download"]').isDisabled(),true);
 await page.keyboard.press('Escape');await page.evaluate(()=>{records[0].status='in_progress';});
 await page.locator('#toggle-settings').click();await page.locator('#capture-types').fill('zip cbz');await page.locator('#capture-excluded').fill('*.example.org');await page.locator('#capture-save').click();await page.waitForFunction(()=>captureRules.extensions.includes('cbz'));await page.locator('#capture-default').click();await page.locator('#capture-save').click();await page.waitForFunction(()=>captureRules.extensions.includes('pdf'));assert.equal(await page.evaluate(()=>captureRules.extensions.includes('cbz')),false);await page.locator('#settings-connections').selectOption('16');await page.locator('#settings-container').selectOption('mp3');await page.locator('#save-settings').click();assert.equal(await page.evaluate(()=>featureSettings.transfer.connections),16);assert.equal(await page.evaluate(()=>featureSettings.transfer.container),'mp3');
 await page.locator('#toggle-settings').click();
 await page.locator('#settings-start-on-login').check();await page.locator('#save-settings').click();assert.equal(await page.evaluate(()=>startOnLogin),true);
 await page.locator('#toggle-settings').click();assert.equal(await page.locator('#settings-start-on-login').isChecked(),true);
 await page.locator('#settings-start-on-login').uncheck();await page.locator('#save-settings').click();assert.equal(await page.evaluate(()=>startOnLogin),false);
 await page.locator('#toggle-settings').click();
 assert.equal(await page.locator('#settings-bandwidth-limit').isDisabled(),true);
 await page.locator('#settings-speed-mode').selectOption('limited');
 await page.locator('#settings-bandwidth-limit').fill('1000');
 await page.locator('#settings-speed-unit').selectOption('mbs');
 assert.equal(Number(await page.locator('#settings-bandwidth-limit').inputValue()),125);
 await page.locator('#save-settings').click();
 assert.equal(await page.evaluate(()=>window.speedLimit),122070);
 await page.locator('#toggle-settings').click();
 assert.equal(await page.locator('#settings-speed-mode').inputValue(),'limited');
 assert.ok(Math.abs(Number(await page.locator('#settings-bandwidth-limit').inputValue())-1000)<0.01);
 await page.locator('#settings-speed-mode').selectOption('unlimited');await page.locator('#save-settings').click();
 assert.equal(await page.evaluate(()=>window.speedLimit),0);
 await page.goto('http://127.0.0.1:18763/ui/add-download.html');await page.locator('#url').fill('https://example.com/one.zip\nhttps://example.com/fail.zip\nhttps://example.com/one.zip');await page.locator('#connections').selectOption('4');await page.locator('#download').click();await page.waitForFunction(()=>!document.getElementById('url').disabled);assert.equal(await page.locator('#url').inputValue(),'https://example.com/fail.zip');assert.equal(await page.evaluate(()=>calls.filter(c=>c.cmd==='start_download').length),2);assert.equal(await page.evaluate(()=>calls.find(c=>c.cmd==='start_download').args.transfer.connections),4);
 await page.goto('http://127.0.0.1:18763/ui/download-detail.html');await page.locator('#segment-strip i').first().waitFor();assert.equal(await page.locator('#segment-strip i').count(),8);assert.notEqual(await page.locator('#segment-strip i').first().evaluate(e=>e.style.width),await page.locator('#segment-strip i').last().evaluate(e=>e.style.width));await page.locator('#auto-close-detail').check();await page.waitForFunction(()=>featureSettings.autoCloseDetail);await page.setViewportSize({width:680,height:850});
 await page.locator('#minimize').click();assert.equal(await page.evaluate(()=>calls.filter(c=>c.cmd==='minimize_download_detail').length),1);assert.equal(await page.evaluate(()=>calls.filter(c=>c.cmd==='pause_download'||c.cmd==='cancel_download').length),0);
 for(const status of ['paused','failed','cancelled']){
  await page.evaluate(status=>{records[0].status=status;},status);
  await page.waitForFunction(()=>document.querySelector('#pause-resume').textContent==='Resume');
  assert.equal(await page.locator('#pause-resume').isEnabled(),true);
  const before=await page.evaluate(()=>calls.filter(c=>c.cmd==='resume_download').length);
  await page.locator('#pause-resume').click();
  assert.equal(await page.evaluate(()=>calls.filter(c=>c.cmd==='resume_download'&&c.args.id===1).length),before+1);
 }
 assert.equal(await page.locator('#cancel-download').isDisabled(),true);
 await page.evaluate(()=>{records[0].status='completed';});
 await page.waitForFunction(()=>document.querySelector('#pause-resume').disabled);
 await page.evaluate(()=>{records[0].status='in_progress';});
 await page.waitForFunction(()=>!document.querySelector('#pause-resume').disabled);
 await page.locator('#toggle-details').click();assert.equal(await page.locator('#connection-details').isVisible(),false);await page.locator('#toggle-details').click();assert.equal(await page.locator('#connection-details').isVisible(),true);
 assert.equal(await page.locator('html').evaluate(e=>getComputedStyle(e).getPropertyValue('--accent').trim()),'rgb(30, 130, 90)');
 for(const size of [{width:520,height:460},{width:680,height:850}]){await page.setViewportSize(size);assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);}
 await page.evaluate(()=>{testPalette={bg:'rgb(239, 241, 245)',surface:'rgb(239, 241, 245)',text:'rgba(17, 17, 27, 0.87)',accent:'rgb(136, 57, 239)'};window.dispatchEvent(new Event('focus'));});
 await page.waitForFunction(()=>document.documentElement.style.getPropertyValue('--accent')==='rgb(136, 57, 239)');
 if(process.env.LDM_SCREENSHOT)await page.screenshot({path:process.env.LDM_SCREENSHOT,fullPage:true});
 await page.evaluate(()=>window.mediaProgress={parallelLimit:10,completedFragments:17,totalFragments:40});
 await page.locator('#media-transfer').waitFor({state:'visible'});
 assert.equal(await page.locator('#connection-table').isVisible(),false);
 assert.equal(await page.locator('#segment-strip').isVisible(),false);
 assert.ok((await page.locator('#media-fragments').textContent()).includes('17 / 40'));
 assert.ok((await page.locator('#media-parallel').textContent()).includes('10'));
 await page.evaluate(()=>window.mediaProgress={parallelLimit:10,completedFragments:17,totalFragments:40,parts:Array.from({length:10},(_,index)=>({index,label:'Akış parçası '+(index+1),status:'receiving',downloadedBytes:(index+1)*100,totalBytes:1000,start:0}))});
 await page.waitForFunction(()=>document.querySelectorAll('#segment-rows tr').length===10);
 assert.equal(await page.locator('#connection-table').isVisible(),true);
 assert.equal(await page.locator('#segment-strip i').count(),10);
 assert.notEqual(await page.locator('#segment-strip i').first().evaluate(e=>e.style.width),await page.locator('#segment-strip i').last().evaluate(e=>e.style.width));
 if(process.env.LDM_MEDIA_SCREENSHOT)await page.screenshot({path:process.env.LDM_MEDIA_SCREENSHOT,fullPage:true});
 await page.evaluate(()=>window.mediaProgress=null);
 await page.locator('#connection-table').waitFor({state:'visible'});
 assert.deepEqual(errors,[]);await browser.close();console.log('PASS: sorting, persistent preferences, deduplicated batch with partial failure, detail segment progress and auto-close setting');
})().catch(e=>{console.error(e);process.exit(1)});
