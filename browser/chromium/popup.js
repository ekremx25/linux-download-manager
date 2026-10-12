const $=id=>document.getElementById(id);
const HOST='com.eko.linuxdownloadmanager';let tab=null,busy=false,polling=false;
const sendNative=payload=>new Promise(resolve=>chrome.runtime.sendNativeMessage(HOST,payload,result=>resolve(chrome.runtime.lastError?{ok:false,error:chrome.runtime.lastError.message}:result)));
const control=payload=>chrome.runtime.sendMessage({type:'popup-control',tabId:tab?.id,...payload});
function error(message){$('error').hidden=!message;$('error').textContent=message||'';}
function size(n){if(n==null)return '—';let i=0;while(n>=1024&&i<4){n/=1024;i++;}return `${n.toFixed(i?1:0)} ${['B','KB','MB','GB','TB'][i]}`;}
function node(tag,cls,text){const e=document.createElement(tag);if(cls)e.className=cls;if(text!=null)e.textContent=text;return e;}
async function action(payload){busy=true;try{const result=await sendNative(payload);if(!result?.ok)throw new Error(result?.error||'Application unavailable');error('');return true;}catch(e){error(String(e));return false;}finally{busy=false;}}
async function refresh(){
 if(polling||busy)return;polling=true;
 try{const result=await sendNative({action:'status'});if(!result?.ok)throw new Error(result?.error||'Application unavailable');$('connection').textContent='● Connected to Linux Download Manager';
 const rows=result.downloads||[],list=$('download-list');$('no-downloads').hidden=rows.length>0;
 const ids=rows.map(r=>r.id+':'+r.status).join('|');
 if(list.dataset.ids!==ids){list.replaceChildren();for(const r of rows){const row=node('div','download');row.dataset.id=r.id;const info=node('div','info');info.append(node('div','name',r.name));const bar=node('div','bar');bar.append(node('i'));info.append(bar,node('div','meta'));row.append(info);const actions=node('div','actions');const paused=['paused','failed','cancelled'].includes(r.status);const toggle=node('button','',paused?'▶':'Ⅱ');toggle.title=paused?'Resume':'Pause';toggle.onclick=async()=>{toggle.disabled=true;await action({action:paused?'resume':'pause',id:r.id});toggle.disabled=false;refresh();};const cancel=node('button','','✕');cancel.title='Cancel';cancel.onclick=async()=>{cancel.disabled=true;await action({action:'cancel',id:r.id});cancel.disabled=false;refresh();};actions.append(toggle,cancel);row.append(actions);list.append(row);}list.dataset.ids=ids;}
 for(const r of rows){const row=list.querySelector(`[data-id="${r.id}"]`);const percent=r.totalBytes?Math.min(100,100*r.downloadedBytes/r.totalBytes):0;row.querySelector('i').style.width=percent+'%';row.querySelector('.meta').textContent=`${r.status} · ${size(r.downloadedBytes)} / ${size(r.totalBytes)}${r.speedBytesPerSecond?' · '+size(r.speedBytesPerSecond)+'/s':''}`;}
 }catch(e){$('connection').textContent='○ Not connected — restart the app after an update';error(String(e));}finally{polling=false;}
}
async function media(){const data=await control({});if(!data?.ok){error(data?.error||'Could not read extension state');return;}const list=$('media-list');list.replaceChildren();const rows=data.media||[];$('no-media').hidden=rows.length>0;for(const m of rows){const row=node('div','media-row');const name=node('div','info',m.url);name.style.overflowWrap='anywhere';const button=node('button','','Download');button.onclick=async()=>{button.disabled=true;const cookies=await chrome.cookies.getAll({url:m.url}).catch(()=>[]);const ok=await action({url:m.url,streamManifest:m.streamManifest,sourcePageUrl:m.referrerUrl||tab?.url,httpHeaders:{referer:m.referrerUrl||tab?.url||'',cookie:cookies.map(c=>c.name+'='+c.value).join('; ')}});button.disabled=false;if(ok)button.textContent='Added';};row.append(name,button);list.append(row);}}
let defaultExtensions=[];
function parseExtensions(text){const parts=text.toLowerCase().split(/[\s,;]+/).filter(Boolean).map(x=>x.replace(/^\./,''));if(parts.some(x=>! /^[a-z0-9]{1,20}$/.test(x)))throw new Error('Enter extensions such as zip, mp4, pdf.');const values=[...new Set(parts)];if(values.length>300)throw new Error('You can add up to 300 file types.');return values;}
function updateTypeCount(){try{$('extension-count').textContent=parseExtensions($('extensions').value).length+' file types';}catch{$('extension-count').textContent='The list contains an invalid extension.';}}
function addExtensions(){const values=parseExtensions($('extensions').value+' '+$('custom').value);$('extensions').value=values.join(', ');$('custom').value='';updateTypeCount();}
async function loadMediaPreferences(){const result=await sendNative({action:'media_preferences'});if(!result?.ok){$('save-media').disabled=true;error(result?.error||'Could not load preferences. Update and restart LDM.');return;}const p=result.preferences;$('media-container').value=p.container;$('media-height').value=String(p.maxHeight);updateMediaFields();$('save-media').disabled=false;}
function updateMediaFields(){$('media-height').disabled=['mp3','m4a'].includes($('media-container').value);}
$('media-container').onchange=updateMediaFields;
$('save-media').onclick=async()=>{const b=$('save-media');b.disabled=true;try{const result=await sendNative({action:'media_preferences',preferences:{container:$('media-container').value,maxHeight:Number($('media-height').value)}});if(!result?.ok)throw new Error(result?.error||'Could not save');$('media-saved').textContent='Saved. Choose “Use saved preferences” in the video download menu.';error('');}catch(e){error(String(e));}finally{b.disabled=false;}};
async function init(){[tab]=await chrome.tabs.query({active:true,currentWindow:true});const data=await control({});if(data?.ok){$('capture-enabled').checked=data.settings.enabled;defaultExtensions=data.defaults||Object.entries(data.groups).filter(([k])=>k!=='image').flatMap(([,v])=>v.extensions||[]);$('extensions').value=(data.settings.extensions||parseExtensions((data.settings.groups||[]).flatMap(g=>data.groups[g]?.extensions||[]).join(',')+','+(data.settings.custom||''))).join(', ');updateTypeCount();}else error(data?.error||'Could not load capture settings');await loadMediaPreferences();await media();await refresh();}
$('extensions').oninput=()=>{updateTypeCount();$('saved').textContent='Unsaved changes';};
$('add-extension').onclick=()=>{try{addExtensions();error('');$('saved').textContent='Added to the list. Click Save to apply.';}catch(e){error(String(e));}};
$('custom').onkeydown=e=>{if(e.key==='Enter'){e.preventDefault();$('add-extension').click();}};
$('reset-types').onclick=()=>{$('extensions').value=defaultExtensions.join(', ');$('custom').value='';updateTypeCount();$('saved').textContent='Default list restored. Click Save to apply.';error('');};
document.querySelectorAll('[data-tab]').forEach(b=>b.onclick=()=>{document.querySelectorAll('[data-tab]').forEach(x=>{x.classList.toggle('active',x===b);$(x.dataset.tab).hidden=x!==b;});if(b.dataset.tab==='media')media();});
$('save').onclick=async()=>{const button=$('save');button.disabled=true;try{addExtensions();const extensions=parseExtensions($('extensions').value);const data=await control({settings:{enabled:$('capture-enabled').checked,extensions,groups:[],custom:''}});if(!data?.ok)throw new Error(data?.error||'Could not save settings');error('');$('saved').textContent='Saved. Applies immediately to new downloads.';}catch(e){error(String(e));}finally{button.disabled=false;}};
$('open').onclick=()=>action({action:'show'});
$('page-video').onclick=async()=>{if(!tab?.url||!/^https?:/.test(tab.url)){error('Open a video page first.');return;}let transfer;
if(/(^|\.)youtube\.com$/.test(new URL(tab.url).hostname)){
 const selected=await chrome.tabs.sendMessage(tab.id,{type:'selected-audio-language'}).catch(()=>null);
 if(!selected?.language){error('Could not read the selected audio language. Reload YouTube and select an audio track.');return;}
 const prefs=await sendNative({action:'media_preferences'});if(!prefs?.ok){error(prefs?.error||'Could not load preferences');return;}
 transfer={...prefs.preferences,audioLanguage:selected.language};
}
const ok=await action({url:tab.url,forceYtdlp:true,sourcePageUrl:tab.url,sourceTitle:tab.title,transfer});if(ok)$('page-video').textContent='Added to downloads';};
init().catch(e=>error(String(e)));const timer=setInterval(()=>{if(!document.hidden)refresh();},2000);window.addEventListener('unload',()=>clearInterval(timer));
