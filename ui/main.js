import {isTauri,invoke,listen,escapeHtml,formatBytes,formatSpeed,formatTime,progressOf,categoryFor,categoryIcon,statusLabel,setNotice} from './shared.js';

const DEMO_DOWNLOADS=[
  {id:1,fileName:'Linux-IDE-x86_64.AppImage',totalBytes:972450611,downloadedBytes:0,status:'queued'},
  {id:2,fileName:'Guitar.mp3',totalBytes:2222981,downloadedBytes:2222981,status:'completed'},
  {id:3,fileName:'Stories-of-Shahnameh.mp4',totalBytes:1224065679,downloadedBytes:478517248,status:'in_progress'},
  {id:4,fileName:'Phoenix.png',totalBytes:212121,downloadedBytes:212121,status:'completed'},
  {id:5,fileName:'Archive.zip',totalBytes:1111490,downloadedBytes:355676,status:'paused'},
  {id:6,fileName:'my-resume.pdf',totalBytes:1782579,downloadedBytes:1782579,status:'completed'}
];
const state={downloads:[],live:new Map(),filter:'all',query:'',contextMenuId:null};
// Smoothed ETA per download so momentary speed spikes do not make the
// countdown jump around.
const smoothedEta=new Map();
const $=id=>document.getElementById(id);

document.addEventListener('DOMContentLoaded',async()=>{
  bind();
  if(!isTauri){state.downloads=DEMO_DOWNLOADS;state.live.set(3,{speedBytesPerSecond:5151129,etaSeconds:142});render();return;}
  await Promise.all([loadSettings(),loadDownloads()]);
  // Polling must not depend on the event subscription: without the core
  // event permission `listen` rejects and the list would freeze at startup.
  setInterval(loadDownloads,5000);
  if(typeof window!=='undefined'&&window.addEventListener)window.addEventListener('focus',loadDownloads);
  try{await listen('download://state',event=>{state.live.set(event.payload.id,event.payload); if(['completed','failed','cancelled'].includes(event.payload.status)) loadDownloads(); else updateRow(event.payload.id);});}
  catch(error){setNotice($('notice'),'Live progress updates unavailable; the list still refreshes every 5 seconds.','error');}
});

function bind(){
  $('add-url').onclick=openAdd; document.querySelectorAll('[data-open-add]').forEach(x=>x.onclick=openAdd);
  $('search-input').oninput=e=>{state.query=e.target.value.toLowerCase();render();};
  document.querySelectorAll('[data-filter]').forEach(button=>button.onclick=()=>selectFilter(button.dataset.filter));
  $('clear-completed').onclick=async()=>{await safe(()=>invoke('clear_completed'));await loadDownloads();};
  $('toggle-settings').onclick=()=>{$('settings-panel').hidden=!$('settings-panel').hidden;};
  $('save-settings').onclick=saveSettings;
  $('open-queues').onclick=()=>selectFilter('unfinished');
  $('start-queue').onclick=()=>batch(['paused','failed'],'resume_download');
  $('stop-queue').onclick=()=>batch(['in_progress'],'pause_download');
  $('stop-all').onclick=()=>batch(['in_progress','queued'],'pause_download');
  document.addEventListener('contextmenu',event=>{
    const row=event.target.closest?.('.download-row.item');
    if(!row)return;
    event.preventDefault();
    openContextMenu(Number(row.dataset.id),event.clientX,event.clientY);
  });
  document.addEventListener('click',event=>{
    if(!event.target.closest?.('#context-menu'))hideContextMenu();
  });
  document.addEventListener('keydown',event=>{
    if(event.key==='Escape')hideContextMenu();
    if(event.ctrlKey&&event.key.toLowerCase()==='n'){event.preventDefault();openAdd();return;}
    if(['ArrowDown','ArrowUp'].includes(event.key)&&!event.target.isContentEditable&&!['INPUT','SELECT','TEXTAREA'].includes(event.target.tagName)){
      event.preventDefault();focusAdjacentRow(event.key==='ArrowDown'?1:-1);
    }
  });
}
async function openAdd(){await safe(()=>invoke('show_add_download_window'));}
async function safe(operation){try{return await operation();}catch(error){setNotice($('notice'),String(error),'error');}}
async function loadSettings(){const s=await safe(()=>invoke('app_settings'));if(!s)return;$('settings-max-concurrent').value=s.maxConcurrentDownloads;$('settings-bandwidth-limit').value=s.defaultBandwidthLimitKbps||'';}
async function saveSettings(){await safe(()=>invoke('update_app_settings',{maxConcurrentDownloads:Number($('settings-max-concurrent').value),defaultBandwidthLimitKbps:Number($('settings-bandwidth-limit').value)||0}));$('settings-panel').hidden=true;setNotice($('notice'),'Settings saved.');}
async function loadDownloads(){const list=await safe(()=>invoke('list_downloads'));if(list){state.downloads=list;render();}}
async function batch(statuses,command){const targets=state.downloads.filter(d=>statuses.includes(d.status));for(const d of targets)await safe(()=>invoke(command,{id:d.id}));await loadDownloads();}
function selectFilter(filter){state.filter=filter;document.querySelectorAll('[data-filter]').forEach(b=>b.classList.toggle('active',b.dataset.filter===filter));$('view-description').textContent=filter==='all'?'All files':filter[0].toUpperCase()+filter.slice(1);render();}
function filtered(){return state.downloads.filter(d=>{const cat=categoryFor(d);const matchFilter=state.filter==='all'||state.filter===cat||(state.filter==='finished'&&d.status==='completed')||(state.filter==='unfinished'&&d.status!=='completed'&&d.status!=='cancelled');return matchFilter&&(!state.query||`${d.fileName} ${d.status}`.toLowerCase().includes(state.query));});}
function render(){const focused=document.activeElement;const focusedRow=focused?.closest?.('.download-row.item');const focusId=focusedRow?.dataset.id;const selector=focused===focusedRow?null:focused?.matches?.('input[type="checkbox"]')?'input[type="checkbox"]':focused?.getAttribute?.('data-action')?`[data-action="${focused.dataset.action}"]`:focused?.getAttribute?.('data-detail')!==null&&focusedRow?'[data-detail]':null;const list=filtered();$('count-all').textContent=state.downloads.length;$('download-rows').innerHTML=list.map(rowHtml).join('');$('empty-state').hidden=list.length>0;attachRows();if(focusId){const replacement=[...document.querySelectorAll('.download-row.item')].find(row=>row.dataset.id===focusId);if(replacement)(selector?replacement.querySelector(selector)||replacement:replacement).focus();}const active=state.downloads.filter(d=>d.status==='in_progress');const speed=active.reduce((n,d)=>n+(state.live.get(d.id)?.speedBytesPerSecond||0),0);$('selection-summary').textContent=`${list.length} of ${state.downloads.length} downloads`;$('queue-summary').textContent=active.length?`${active.length} active · ${formatSpeed(speed)}`:'Idle';}
function rowHtml(d){const live=state.live.get(d.id)||{};const merged={...d,...live};const progress=progressOf(merged);const cat=categoryFor(d);const canPause=['in_progress','queued'].includes(d.status);const canResume=['paused','failed'].includes(d.status);return `<div class="download-row item" role="row" data-id="${d.id}" tabindex="0"><input type="checkbox" aria-label="Select ${escapeHtml(d.fileName)}"><div class="file-cell"><span class="file-icon">${categoryIcon(cat)}</span><div><div class="file-name" title="${escapeHtml(d.fileName)}">${escapeHtml(d.fileName)}</div><div class="file-kind">${cat}</div></div></div><span>${formatBytes(d.totalBytes||d.downloadedBytes)}</span><div class="status-cell"><div class="status-line"><span data-cell="status">${progress.toFixed(0)}% ${statusLabel(merged.status)}</span></div><div class="progress ${merged.status}" data-cell="progress-track"><i data-cell="progress" style="width:${progress}%"></i></div></div><span data-cell="speed">${speedFor(d,live)}</span><span data-cell="eta">${etaFor(d,live)}</span><div class="row-actions">${canPause?`<button class="icon-button" data-action="pause_download" title="Pause">Ⅱ</button>`:''}${canResume?`<button class="icon-button" data-action="resume_download" title="Resume">▶</button>`:''}<button class="icon-button" data-detail title="Details">ⓘ</button></div></div>`;}
function attachRows(){document.querySelectorAll('.download-row.item').forEach(row=>{const id=Number(row.dataset.id);row.ondblclick=()=>openDetail(id);row.onkeydown=e=>{if(e.key==='Enter'&&e.target===row){e.preventDefault();openDetail(id);}};row.querySelector('[data-detail]').onclick=e=>{e.stopPropagation();openDetail(id);};row.querySelectorAll('[data-action]').forEach(button=>button.onclick=async e=>{e.stopPropagation();await safe(()=>invoke(button.dataset.action,{id}));await loadDownloads();});});}
function focusAdjacentRow(direction){const rows=[...document.querySelectorAll('.download-row.item')];if(!rows.length)return;const current=rows.indexOf(document.activeElement.closest?.('.download-row.item'));const next=current<0?(direction>0?0:rows.length-1):(current+direction+rows.length)%rows.length;rows[next].focus();}
async function openDetail(id){await safe(()=>invoke('show_download_detail_window',{id}));}

// Speed comes from the backend progress event; never a placeholder value.
function speedFor(d,live){
  if(d.status!=='in_progress')return '—';
  return formatSpeed(live.speedBytesPerSecond||0);
}
// ETA prefers the backend value, falls back to remaining/speed, and is
// exponentially smoothed so a single slow sample cannot spike the countdown.
function etaFor(d,live){
  if(d.status!=='in_progress'){smoothedEta.delete(d.id);return '—';}
  const speed=live.speedBytesPerSecond||0;
  const total=live.totalBytes??d.totalBytes??0;
  const done=live.downloadedBytes??d.downloadedBytes??0;
  let eta=live.etaSeconds;
  if(!eta&&speed>0)eta=Math.max(0,total-done)/speed;
  if(!eta||eta<=0)return '—';
  const previous=smoothedEta.get(d.id);
  const next=previous==null?eta:previous*0.6+eta*0.4;
  smoothedEta.set(d.id,next);
  return formatTime(Math.round(next));
}
// Progress events must only patch the one row that changed, never the list.
function updateRow(id){
  const row=document.querySelector(`.download-row.item[data-id="${id}"]`);
  const d=state.downloads.find(item=>item.id===id);
  if(!row||!d)return false;
  const live=state.live.get(id)||{};
  const merged={...d,...live};
  const progress=progressOf(merged);
  const status=row.querySelector('[data-cell="status"]');
  if(status)status.textContent=`${progress.toFixed(0)}% ${statusLabel(merged.status)}`;
  const track=row.querySelector('[data-cell="progress-track"]');
  const bar=row.querySelector('[data-cell="progress"]');
  if(track)track.className=`progress ${merged.status}`;
  if(bar)bar.style.width=`${progress}%`;
  const speed=row.querySelector('[data-cell="speed"]');
  if(speed)speed.textContent=speedFor(d,live);
  const eta=row.querySelector('[data-cell="eta"]');
  if(eta)eta.textContent=etaFor(d,live);
  updateQueueSummary();
  return true;
}
function updateQueueSummary(){
  const active=state.downloads.filter(d=>d.status==='in_progress');
  const speed=active.reduce((total,d)=>total+(state.live.get(d.id)?.speedBytesPerSecond||0),0);
  const summary=$('queue-summary');
  if(summary)summary.textContent=active.length?`${active.length} active · ${formatSpeed(speed)}`:'Idle';
}
async function clearDownload(id){
  const d=state.downloads.find(item=>item.id===id);
  if(!d)return;
  // Active transfer is stopped first so the queue cannot re-add the row.
  if(['in_progress','queued','scheduled'].includes(d.status))await safe(()=>invoke('pause_download',{id}));
  state.downloads=state.downloads.filter(item=>item.id!==id);
  state.live.delete(id);smoothedEta.delete(id);
  render();
  await safe(()=>invoke('clear_download',{id}));
}
const CONTEXT_MENU_ITEMS=[['back','Back'],['forward','Forward'],['stop','Stop'],['clear','Clear'],['reload','Reload'],null,['inspect','Inspect Element']];
function ensureContextMenu(){
  let menu=$('context-menu');
  if(menu)return menu;
  menu=document.createElement('div');
  menu.id='context-menu';
  menu.className='context-menu';
  menu.hidden=true;
  menu.setAttribute('role','menu');
  menu.innerHTML=CONTEXT_MENU_ITEMS.map(item=>item===null?'<div class="context-menu-separator"></div>':`<button type="button" role="menuitem" data-menu-action="${item[0]}">${item[1]}</button>`).join('');
  document.body.appendChild(menu);
  menu.addEventListener('click',async event=>{
    const button=event.target.closest('[data-menu-action]');
    if(!button)return;
    const action=button.dataset.menuAction;
    const id=state.contextMenuId;
    hideContextMenu();
    if(action==='clear')await clearDownload(id);
    else if(action==='stop')await safe(()=>invoke('pause_download',{id}));
    else if(action==='back')history.back();
    else if(action==='forward')history.forward();
    else if(action==='reload')location.reload();
    else if(action==='inspect')setNotice($('notice'),'Inspect Element is provided by the WebKit inspector.','info');
  });
  return menu;
}
function openContextMenu(id,x,y){
  const menu=ensureContextMenu();
  state.contextMenuId=id;
  menu.hidden=false;
  const width=menu.offsetWidth||200;
  const height=menu.offsetHeight||220;
  menu.style.left=`${Math.min(x,window.innerWidth-width-6)}px`;
  menu.style.top=`${Math.min(y,window.innerHeight-height-6)}px`;
}
function hideContextMenu(){
  const menu=$('context-menu');
  if(menu)menu.hidden=true;
  state.contextMenuId=null;
}
