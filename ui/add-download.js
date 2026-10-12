import {isTauri,invoke,formatBytes,setNotice} from './shared.js';
const $=id=>document.getElementById(id);
let metadata=null, inspectedUrl='', generation=0, submitting=false;
const urls=()=>[...new Set($('url').value.split(/\r?\n/).map(s=>s.trim()).filter(Boolean))];
const controls=['url','paste','save-dir','choose-dir','file-name','checksum','schedule','bandwidth','enqueue-only','inspect','download','cancel','close','as-video','connections','container','height'];
document.addEventListener('DOMContentLoaded',()=>{
  $('close').onclick=$('cancel').onclick=closeWindow;
  $('url').oninput=invalidate;$('as-video').onchange=invalidate;
  $('paste').onclick=async()=>{const version=generation;try{const text=await navigator.clipboard.readText();if(submitting||version!==generation)return;$('url').value=text;invalidate();}catch{setNotice($('add-notice'),'Use Ctrl+V to paste links.','error');}};
  $('choose-dir').onclick=async()=>{try{const path=await invoke('pick_save_directory');if(path&&!submitting)$('save-dir').value=path;}catch(e){setNotice($('add-notice'),String(e),'error');}};
  $('inspect').onclick=inspect;$('download').onclick=start;
  $('enqueue-only').onchange=$('schedule').oninput=refresh;
  document.addEventListener('keydown',e=>{if(e.key==='Escape')closeWindow();if(e.ctrlKey&&e.key==='Enter'){e.preventDefault();start();}});
  refresh();
});
function invalidate(){generation++;metadata=null;inspectedUrl='';$('metadata').hidden=true;$('file-name').value='';refresh();}
function refresh(){
  const count=urls().length, single=count===1;
  $('download').disabled=submitting||count===0;
  $('download').textContent=submitting?'Adding…':$('enqueue-only').checked?'Add to queue':$('schedule').value?'Schedule':count>1?`Add ${count} downloads`:'Start download';
  $('inspect').disabled=submitting||!single||$('as-video').checked;
  $('file-name').disabled=$('checksum').disabled=submitting||!single;
}
async function closeWindow(){if(!submitting)await invoke('hide_app_window',{label:'add-download'});}
function validUrl(value){try{const u=new URL(value);return ['http:','https:'].includes(u.protocol)&&!!u.hostname;}catch{return false;}}
async function inspect(){
  if(submitting||urls().length!==1)return;
  const url=urls()[0], version=++generation;
  if(!validUrl(url)){setNotice($('add-notice'),'Enter an HTTP or HTTPS URL.','error');return;}
  $('inspect').disabled=true;
  try{const result=await invoke('inspect_url',{url});if(version!==generation)return;metadata=result;inspectedUrl=url;$('file-name').value=result.suggestedFileName;$('meta-size').textContent=formatBytes(result.contentLength);$('meta-type').textContent=result.contentType||'Unknown';$('meta-resume').textContent=result.resumable?'Yes':'No';$('metadata').hidden=false;}
  catch(e){setNotice($('add-notice'),String(e),'error');}finally{if(version===generation)refresh();}
}
async function start(){
  if(submitting)return;const list=urls();if(!list.length)return;
  if(list.length>100){setNotice($('add-notice'),'Add up to 100 URLs at a time.','error');return;}
  if(!$('bandwidth').reportValidity())return;
  const fileName=list.length===1?$('file-name').value.trim():'';
  if(fileName&&(/[\\/\x00-\x1f\x7f:*?"<>|]/.test(fileName)||['.','..'].includes(fileName))){setNotice($('add-notice'),'Use a file name without path separators or reserved characters.','error');return;}
  submitting=true;generation++;controls.forEach(id=>$(id).disabled=true);refresh();
  const failed=[], results=[];
  try {
    const settings=await invoke('app_settings');const transfer={...(settings.features?.transfer||{connections:0,maxHeight:0,container:'mp4'})};
    if($('connections').value!=='')transfer.connections=Number($('connections').value);
    if($('height').value!=='')transfer.maxHeight=Number($('height').value);
    if($('container').value!=='')transfer.container=$('container').value;
    for(const url of list){
      setNotice($('add-notice'),`Adding ${results.length+1} / ${list.length}…`);
      try{
        if(!validUrl(url))throw new Error('Expected an HTTP or HTTPS URL');
        await invoke('start_download',{url,fileName:fileName||null,transfer,asVideo:$('as-video').checked,enqueueOnly:$('enqueue-only').checked,saveDir:$('save-dir').value.trim()||null,expectedChecksum:list.length===1?$('checksum').value.trim()||null:null,scheduledAt:$('schedule').value||null,bandwidthLimitKbps:$('bandwidth').value===''?null:Number($('bandwidth').value)});
        results.push(`✓ ${url}`);
      }catch(e){failed.push(url);results.push(`✕ ${url} — ${String(e)}`);}
    }
    $('batch-results').hidden=false;$('batch-results').textContent=results.join('\n');
    $('url').value=failed.join('\n');metadata=null;inspectedUrl='';
    setNotice($('add-notice'),`${list.length-failed.length} added; ${failed.length} failed.`,failed.length?'error':'success');
    if(list.length===1&&!failed.length){$('file-name').value='';$('checksum').value='';await invoke('hide_app_window',{label:'add-download'});}
  }catch(e){setNotice($('add-notice'),String(e),'error');}
  finally{submitting=false;controls.forEach(id=>$(id).disabled=false);refresh();}
}
