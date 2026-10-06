import {isTauri,invoke,formatBytes,setNotice} from './shared.js';
const $=id=>document.getElementById(id);
function focusUrl(){try{const input=$('url');if(input&&!input.disabled)input.forceActiveFocus();}catch{}}
let metadata=null, inspectedUrl='', generation=0, submitting=false;
document.addEventListener('DOMContentLoaded',async()=>{
  bind();
  if(!isTauri){$('url').value='https://download.example.org/releases/linux-downloader.tar.zst';$('save-dir').value='~/Downloads';metadata={suggestedFileName:'linux-downloader.tar.zst',contentLength:381000000,contentType:'application/zstd',resumable:true};inspectedUrl=$('url').value;$('file-name').value=metadata.suggestedFileName;$('meta-size').textContent=formatBytes(metadata.contentLength);$('meta-type').textContent=metadata.contentType;$('meta-resume').textContent='Yes';$('metadata').hidden=false;$('download').disabled=false;return;}
  const settings=await invoke('app_settings').catch(()=>null);
  if(settings)$('save-dir').placeholder=settings.defaultDownloadDir;
});
function invalidate(){generation++;metadata=null;inspectedUrl='';$('metadata').hidden=true;$('file-name').value='';setBusy(false,'Inspect');}
function bind(){
  $('close').onclick=$('cancel').onclick=closeWindow;
  document.addEventListener('keydown',event=>{if(event.key==='Escape')closeWindow();});
  $('url').oninput=invalidate;
  $('paste').onclick=async()=>{const version=generation;try{const text=await navigator.clipboard.readText();if(submitting||version!==generation)return;$('url').value=text;invalidate();}catch{setNotice($('add-notice'),'Clipboard access was denied.','error');}};
  $('choose-dir').onclick=chooseDir;$('inspect').onclick=inspect;$('download').onclick=start;
  $('enqueue-only').onchange=()=>{$('download').textContent=$('enqueue-only').checked?'Add to queue':$('schedule').value?'Schedule download':'Download now';};
  $('schedule').oninput=$('enqueue-only').onchange;
  $('url').onkeydown=e=>{if(e.key==='Enter'){e.preventDefault();inspect();}};
}
async function closeWindow(){if(submitting)return;invalidate();await invoke('hide_app_window',{label:'add-download'});}
async function chooseDir(){try{const path=await invoke('pick_save_directory');if(path&&!submitting)$('save-dir').value=path;}catch(error){setNotice($('add-notice'),String(error),'error');}}
async function inspect(){
  if(submitting)return;
  const url=$('url').value.trim();invalidate();
  if(!url){setNotice($('add-notice'),'Enter a download URL.','error');return;}
  const version=generation;setBusy(true,'Inspecting…');
  try{
    const result=await invoke('inspect_url',{url});
    if(version!==generation||url!==$('url').value.trim())return;
    metadata=result;inspectedUrl=url;$('file-name').value=result.suggestedFileName;
    $('meta-size').textContent=result.contentLength==null?'Unknown':formatBytes(result.contentLength);
    $('meta-type').textContent=result.contentType||'Unknown';$('meta-resume').textContent=result.resumable?'Yes':'No';
    $('metadata').hidden=false;$('download').disabled=false;setNotice($('add-notice'),'Link is ready to download.');
  }catch(error){if(version===generation)setNotice($('add-notice'),String(error),'error');}
  finally{if(version===generation)setBusy(false,'Inspect');}
}
function setBusy(busy,label){$('inspect').disabled=busy;$('inspect').textContent=label;$('download').disabled=busy;focusUrl();}
async function start(){
  const url=$('url').value.trim();
  if(submitting||!url)return;
  if(!metadata||url!==inspectedUrl){await inspect();if(!metadata||url!==inspectedUrl)return;}
  const fileName=$('file-name').value.trim();
  if(!fileName||fileName==='.'||fileName==='..'||/[\\/\x00-\x1f\x7f:*?"<>|]/.test(fileName)){setNotice($('add-notice'),'Enter a file name without path separators or reserved characters.','error');return;}
  submitting=true;generation++;
  const controls=['url','paste','save-dir','choose-dir','file-name','checksum','schedule','bandwidth','enqueue-only','inspect','download','cancel','close'];
  controls.forEach(id=>$(id).disabled=true);$('download').textContent='Adding…';
  let added=false;
  try{
    await invoke('start_download',{url,fileName,enqueueOnly:$('enqueue-only').checked,saveDir:$('save-dir').value.trim()||null,expectedChecksum:$('checksum').value.trim()||null,scheduledAt:$('schedule').value||null,bandwidthLimitKbps:Number($('bandwidth').value)||null});
    added=true;reset();await invoke('hide_app_window',{label:'add-download'});
  }catch(error){setNotice($('add-notice'),added?`Download added, but window could not close: ${error}`:String(error),'error');}
  finally{submitting=false;controls.forEach(id=>$(id).disabled=false);$('enqueue-only').onchange();$('download').disabled=!metadata;}
}
function reset(){invalidate();['url','save-dir','checksum','bandwidth','schedule'].forEach(id=>$(id).value='');$('enqueue-only').checked=false;}
