import {isTauri,invoke,listen,escapeHtml,formatBytes,formatSpeed,formatTime,progressOf,categoryFor,categoryIcon,statusLabel,setNotice} from './shared.js';

const DEMO_DOWNLOADS=[
  {id:1,fileName:'Linux-IDE-x86_64.AppImage',category:'apps',totalBytes:972450611,downloadedBytes:0,status:'queued'},
  {id:2,fileName:'Guitar.mp3',category:'music',totalBytes:2222981,downloadedBytes:2222981,status:'completed'},
  {id:3,fileName:'Stories-of-Shahnameh.mp4',category:'video',totalBytes:1224065679,downloadedBytes:478517248,status:'in_progress'},
  {id:4,fileName:'Phoenix.png',category:'image',totalBytes:212121,downloadedBytes:212121,status:'completed'},
  {id:5,fileName:'Archive.zip',category:'compressed',totalBytes:1111490,downloadedBytes:355676,status:'paused'},
  {id:6,fileName:'Presentation.pdf',category:'document',totalBytes:1782579,downloadedBytes:0,status:'failed',errorMessage:'The server closed the connection. You can try again.'}
];
const labels={all:'All files',active:'Downloading',paused:'Paused',failed:'Failed downloads',finished:'Completed',video:'Videos',music:'Music',image:'Images',document:'Documents',compressed:'Archives',apps:'Applications',iso:'ISO',other:'Other'};
const state={downloads:[],live:new Map(),selected:new Set(),filter:'all',query:'',contextMenuId:null};
const smoothedEta=new Map();
const $=id=>document.getElementById(id);
const merged=d=>({...d,...state.live.get(d.id)});
const canPause=d=>['in_progress','queued','scheduled'].includes(d.status);
const canResume=d=>['paused','failed'].includes(d.status);
let listLoading=false,lastList='',actionBusy=false;

document.addEventListener('DOMContentLoaded',async()=>{
  bind();
  if(!isTauri){state.downloads=DEMO_DOWNLOADS;state.live.set(3,{speedBytesPerSecond:5151129,etaSeconds:142});render();return;}
  try{await listen('download://state',event=>{
    const before=state.live.get(event.payload.id)?.status??state.downloads.find(d=>d.id===event.payload.id)?.status;
    state.live.set(event.payload.id,event.payload);
    if(document.hidden)return;
    if(before!==event.payload.status){render();loadDownloads();}else updateRow(event.payload.id);
  });}catch{setNotice($('notice'),'Live updates are unavailable. The list will refresh every 5 seconds.','error');}
  await Promise.all([loadSettings(),loadDownloads()]);
  setInterval(()=>{if(!document.hidden)loadDownloads();},5000);
  window.addEventListener('focus',loadDownloads);
  document.addEventListener('visibilitychange',()=>{if(!document.hidden){render();loadDownloads();}});
});

function bind(){
  $('add-url').onclick=openAdd;
  document.querySelectorAll('[data-open-add]').forEach(b=>b.onclick=openAdd);
  $('search-input').oninput=e=>{state.query=e.target.value.toLocaleLowerCase('en');render();};
  document.querySelectorAll('[data-filter]').forEach(b=>b.onclick=()=>selectFilter(b.dataset.filter));
  $('clear-completed').onclick=async()=>{await safe(()=>invoke('clear_completed'));await loadDownloads();};
  $('toggle-settings').onclick=()=>{const expanded=$('settings-panel').hidden;$('settings-panel').hidden=!expanded;$('toggle-settings').setAttribute('aria-expanded',String(expanded));};
  $('save-settings').onclick=saveSettings;
  $('start-queue').onclick=()=>batch('resume_download');
  $('stop-all').onclick=()=>batch('pause_download');
  $('open-folder').onclick=()=>safe(()=>invoke('open_download_folder',{id:null}));
  $('pause-selected').onclick=()=>batch('pause_download',true);
  $('resume-selected').onclick=()=>batch('resume_download',true);
  $('clear-selection').onclick=()=>{state.selected.clear();render();};
  $('select-all').onchange=e=>{for(const d of filtered()){if(e.target.checked)state.selected.add(d.id);else state.selected.delete(d.id);}render();};
  document.addEventListener('contextmenu',e=>{const row=e.target.closest?.('.download-row.item');if(row){e.preventDefault();openContextMenu(Number(row.dataset.id),e.clientX,e.clientY);}});
  document.addEventListener('click',e=>{if(!e.target.closest?.('#context-menu'))hideContextMenu();});
  document.addEventListener('keydown',e=>{
    if(e.key==='Escape')hideContextMenu();
    if(e.ctrlKey&&e.key.toLowerCase()==='n'){e.preventDefault();openAdd();}
    if(['ArrowDown','ArrowUp'].includes(e.key)&&!e.target.isContentEditable&&!['INPUT','SELECT','TEXTAREA'].includes(e.target.tagName)){e.preventDefault();focusAdjacentRow(e.key==='ArrowDown'?1:-1);}
  });
}
async function openAdd(){await safe(()=>invoke('show_add_download_window'));}
async function safe(fn){try{return await fn();}catch(e){setNotice($('notice'),String(e),'error');}}
async function loadSettings(){const s=await safe(()=>invoke('app_settings'));if(!s)return;$('settings-max-concurrent').value=s.maxConcurrentDownloads;$('settings-bandwidth-limit').value=s.defaultBandwidthLimitKbps||0;$('settings-desktop-notifications').checked=s.desktopNotificationsEnabled!==false;}
async function saveSettings(){
  const count=$('settings-max-concurrent'),limit=$('settings-bandwidth-limit');
  if(!count.reportValidity()||!limit.reportValidity())return;
  $('save-settings').disabled=true;
  try{await invoke('update_app_settings',{maxConcurrentDownloads:Number(count.value),defaultBandwidthLimitKbps:Number(limit.value)||0,desktopNotificationsEnabled:$('settings-desktop-notifications').checked});$('settings-panel').hidden=true;$('toggle-settings').setAttribute('aria-expanded','false');setNotice($('notice'),'Settings saved.');}catch(e){setNotice($('notice'),String(e),'error');}finally{$('save-settings').disabled=false;}
}
async function loadDownloads(){
  if(listLoading||!isTauri)return;listLoading=true;
  try{const list=await invoke('list_downloads');const serialized=JSON.stringify(list);if(serialized!==lastList){lastList=serialized;state.downloads=list;const ids=new Set(list.map(d=>d.id));for(const id of state.selected)if(!ids.has(id))state.selected.delete(id);for(const id of state.live.keys())if(!ids.has(id))state.live.delete(id);render();}}catch(e){setNotice($('notice'),String(e),'error');}finally{listLoading=false;}
}
async function batch(command,selectedOnly=false){
  if(actionBusy)return;actionBusy=true;updateSelection();
  const eligible=command==='pause_download'?canPause:canResume;
  const targets=state.downloads.map(merged).filter(d=>eligible(d)&&(!selectedOnly||state.selected.has(d.id)));
  const failures=[];
  for(const d of targets){try{await invoke(command,{id:d.id});}catch(e){failures.push(`${d.fileName}: ${e}`);}}
  actionBusy=false;await loadDownloads();updateSelection();
  setNotice($('notice'),failures.length?failures.join('\n'):`${targets.length} files updated.`,failures.length?'error':'');
}
function selectFilter(filter){state.filter=filter;document.querySelectorAll('[data-filter]').forEach(b=>b.classList.toggle('active',b.dataset.filter===filter));$('view-description').textContent=labels[filter]||filter;render();}
function filtered(){return state.downloads.map(merged).filter(d=>{
  const match=state.filter==='all'||state.filter===categoryFor(d)||(state.filter==='active'&&['in_progress','queued','scheduled'].includes(d.status))||(state.filter==='finished'&&d.status==='completed')||state.filter===d.status;
  return match&&(!state.query||`${d.fileName} ${statusLabel(d.status)}`.toLocaleLowerCase('en').includes(state.query));
});}
function render(){
  const focused=document.activeElement,oldRow=focused?.closest?.('.download-row.item');
  const id=oldRow?.dataset.id,action=focused?.dataset?.action,isCheck=focused?.matches?.('input[type="checkbox"]');
  const list=filtered();$('count-all').textContent=state.downloads.length;
  $('download-rows').innerHTML=list.map(rowHtml).join('');$('empty-state').hidden=list.length>0;attachRows();
  if(id){const row=document.querySelector(`.download-row.item[data-id="${id}"]`);(isCheck?row?.querySelector('input'):action?row?.querySelector(`[data-action="${action}"]`):row)?.focus();}
  $('selection-summary').textContent=`${list.length} / ${state.downloads.length} files`;
  updateSelection();updateQueueSummary();
}
function progressText(d){return d.status==='in_progress'&&!d.totalBytes?'Calculating size':`${progressOf(d).toFixed(0)}% · ${statusLabel(d.status)}`;}
function rowHtml(d){
  const live=state.live.get(d.id)||{},cat=categoryFor(d),error=d.errorMessage;
  return `<div class="download-row item ${state.selected.has(d.id)?'selected':''}" role="row" data-id="${d.id}" tabindex="0">
  <input type="checkbox" ${state.selected.has(d.id)?'checked':''} aria-label="Select ${escapeHtml(d.fileName)}">
  <div class="file-cell"><span class="file-icon">${categoryIcon(cat)}</span><div><div class="file-name" title="${escapeHtml(d.fileName)}">${escapeHtml(d.fileName)}</div><div class="file-kind ${error?'error-text':''}" title="${escapeHtml(error||'')}">${escapeHtml(error||labels[cat]||'File')}</div></div></div>
  <span data-cell="size" data-label="Size">${formatBytes(d.totalBytes||d.downloadedBytes)}</span>
  <div class="status-cell"><span class="status-line" data-cell="status">${progressText(d)}</span><div class="progress ${d.status}" data-cell="progress-track"><i data-cell="progress" style="width:${progressOf(d)}%"></i></div></div>
  <span data-cell="speed" data-label="Speed">${speedFor(d,live)}</span><span data-cell="eta" data-label="Time left" title="Estimate for the current transfer; video/audio merging time is not included.">${etaFor(d,live)}</span>
  <div class="row-actions">${canPause(d)?'<button class="icon-button" data-action="pause_download" title="Pause" aria-label="Pause">Ⅱ</button>':''}${canResume(d)?'<button class="icon-button" data-action="resume_download" title="Resume / retry" aria-label="Resume / retry">▶</button>':''}<button class="icon-button" data-action="folder" title="Show in folder" aria-label="Show in folder">▱</button><button class="icon-button" data-action="detail" title="Details" aria-label="Details">ⓘ</button></div></div>`;
}
function attachRows(){document.querySelectorAll('.download-row.item').forEach(row=>{
  const id=Number(row.dataset.id);row.ondblclick=e=>{if(!e.target.closest('button,input'))openDetail(id);};
  row.onkeydown=e=>{if(e.key==='Enter'&&e.target===row){e.preventDefault();openDetail(id);}};
  row.querySelector('input').onchange=e=>{if(e.target.checked)state.selected.add(id);else state.selected.delete(id);row.classList.toggle('selected',e.target.checked);updateSelection();};
  row.querySelectorAll('[data-action]').forEach(b=>b.onclick=async()=>{if(b.dataset.action==='detail')return openDetail(id);b.disabled=true;await safe(()=>invoke(b.dataset.action==='folder'?'open_download_folder':b.dataset.action,{id}));b.disabled=false;await loadDownloads();});
});}
function updateSelection(){
  const visible=filtered(),selected=state.downloads.map(merged).filter(d=>state.selected.has(d.id));
  $('selection-bar').hidden=selected.length===0;$('selected-count').textContent=`${selected.length} files selected`;
  $('select-all').checked=visible.length>0&&visible.every(d=>state.selected.has(d.id));$('select-all').indeterminate=visible.some(d=>state.selected.has(d.id))&&!$('select-all').checked;
  $('pause-selected').disabled=actionBusy||!selected.some(canPause);$('resume-selected').disabled=actionBusy||!selected.some(canResume);
  $('stop-all').disabled=actionBusy||!state.downloads.map(merged).some(canPause);$('start-queue').disabled=actionBusy||!state.downloads.map(merged).some(canResume);
}
function focusAdjacentRow(direction){const rows=[...document.querySelectorAll('.download-row.item')];if(!rows.length)return;const current=rows.indexOf(document.activeElement.closest?.('.download-row.item'));const next=current<0?(direction>0?0:rows.length-1):(current+direction+rows.length)%rows.length;rows[next].focus();}
async function openDetail(id){await safe(()=>invoke('show_download_detail_window',{id}));}
function speedFor(d,live){return (live.status??d.status)==='in_progress'?formatSpeed(live.speedBytesPerSecond||0):'—';}
function etaFor(d,live){
  if((live.status??d.status)!=='in_progress'){smoothedEta.delete(d.id);return '—';}
  const speed=live.speedBytesPerSecond||0,total=live.totalBytes??d.totalBytes??0,done=live.downloadedBytes??d.downloadedBytes??0;
  let eta=live.etaSeconds;if(eta==null&&speed>0&&total>done)eta=(total-done)/speed;
  if(!eta||eta<=0)return '—';return formatTime(Math.round(eta));
}
function updateRow(id){
  const row=document.querySelector(`.download-row.item[data-id="${id}"]`),record=state.downloads.find(d=>d.id===id);
  if(!row||!record)return false;const d=merged(record),live=state.live.get(id)||{};
  for(const [cell,value] of Object.entries({status:progressText(d),size:formatBytes(d.totalBytes||d.downloadedBytes),speed:speedFor(d,live),eta:etaFor(d,live)})){const el=row.querySelector(`[data-cell="${cell}"]`);if(el&&el.textContent!==value)el.textContent=value;}
  row.querySelector('[data-cell="progress-track"]').className=`progress ${d.status}`;row.querySelector('[data-cell="progress"]').style.width=`${progressOf(d)}%`;updateQueueSummary();return true;
}
function updateQueueSummary(){
  const all=state.downloads.map(merged),active=all.filter(d=>d.status==='in_progress');
  const speed=active.reduce((n,d)=>n+(state.live.get(d.id)?.speedBytesPerSecond||0),0);
  $('stat-active').textContent=active.length;$('stat-completed').textContent=all.filter(d=>d.status==='completed').length;
  const waiting=all.filter(d=>['queued','scheduled'].includes(d.status)).length;$('stat-waiting').textContent=waiting?`${waiting} files queued`:'Queue is empty';
  $('queue-summary').textContent=active.length?`${active.length} active · ${formatSpeed(speed)}`:'Ready';
}
async function clearDownload(id){
  const d=state.downloads.find(d=>d.id===id);if(!d||canPause(merged(d)))return;
  try{await invoke('clear_download',{id});state.live.delete(id);state.selected.delete(id);lastList='';await loadDownloads();}catch(e){setNotice($('notice'),String(e),'error');}
}
function confirmFileDeletion(fileName){
  return new Promise(resolve=>{
    const dialog=document.createElement('dialog');dialog.className='delete-confirm';
    dialog.innerHTML='<h2>Delete downloaded files?</h2><p class="delete-file-name"></p><p>This permanently deletes the downloaded file and its temporary data, and removes it from the list.</p><form method="dialog"><button value="cancel" autofocus>Cancel</button><button value="delete" class="danger">Delete files</button></form>';
    dialog.querySelector('.delete-file-name').textContent=fileName;
    dialog.addEventListener('close',()=>{const confirmed=dialog.returnValue==='delete';dialog.remove();resolve(confirmed);},{once:true});
    document.body.appendChild(dialog);dialog.showModal();
  });
}
async function deleteDownloadFiles(id){
  const d=state.downloads.find(d=>d.id===id);if(!d||canPause(merged(d)))return;
  if(!await confirmFileDeletion(d.fileName))return;
  try{await invoke('delete_download_files',{id});state.live.delete(id);state.selected.delete(id);lastList='';await loadDownloads();}catch(e){setNotice($('notice'),String(e),'error');}
}
function ensureContextMenu(){
  let menu=$('context-menu');if(menu)return menu;menu=document.createElement('div');menu.id='context-menu';menu.className='context-menu';menu.hidden=true;menu.setAttribute('role','menu');document.body.appendChild(menu);
  menu.addEventListener('click',async e=>{const b=e.target.closest('[data-menu-action]');if(!b||b.disabled)return;const id=state.contextMenuId,action=b.dataset.menuAction;hideContextMenu();if(action==='detail')await openDetail(id);else if(action==='clear')await clearDownload(id);else if(action==='delete-files')await deleteDownloadFiles(id);else await safe(()=>invoke(action==='folder'?'open_download_folder':action,{id}));await loadDownloads();});return menu;
}
function openContextMenu(id,x,y){
  const d=state.downloads.find(d=>d.id===id);if(!d)return;const row=merged(d),menu=ensureContextMenu();state.contextMenuId=id;
  const items=[['detail','Show details',true],['folder','Show in folder',true],['pause_download','Pause',canPause(row)],['resume_download','Resume / retry',canResume(row)],['clear','Remove from list (keep file)',!canPause(row)],['delete-files','Delete files and remove from list',!canPause(row)]];
  menu.innerHTML=items.map(([a,t,enabled])=>`<button role="menuitem" data-menu-action="${a}" ${enabled?'':'disabled'}>${t}</button>`).join('');menu.hidden=false;menu.style.left=`${Math.max(6,Math.min(x,window.innerWidth-menu.offsetWidth-6))}px`;menu.style.top=`${Math.max(6,Math.min(y,window.innerHeight-menu.offsetHeight-6))}px`;
}
function hideContextMenu(){const menu=$('context-menu');if(menu)menu.hidden=true;state.contextMenuId=null;}
