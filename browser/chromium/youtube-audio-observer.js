// Read the selected track without changing playback or guessing from the UI locale.
(() => {
 const safely = fn => { try { return fn(); } catch { return null; } };
 function languageOf(track) {
  if (!track) return null;
  // YouTube also returns track objects whose metadata is exposed through methods.
  const info = safely(() => track.getLanguageInfo());
  const values = [info?.languageCode, info?.language, info?.code, info?.id,
   safely(() => track.getLanguageCode()), track.languageCode, track.language,
   track.audioTrack?.languageCode, track.audioTrack?.id,
   safely(() => track.getId()), track.id, typeof track === 'string' ? track : null];
  for (const value of values) {
   if (typeof value !== 'string') continue;
   const code = value.match(/^([a-z]{2,3}(?:-[a-z0-9]{2,8})*)(?:\.\d+)?$/i)?.[1];
   if (code) return code.toLowerCase();
  }
  return null;
 }
 window.addEventListener('message', event => {
  if (event.source !== window || event.origin !== location.origin || event.data?.type !== 'ldm-audio-track-request') return;
  const short = location.pathname?.startsWith('/shorts/');
  const active = short ? document.querySelector('ytd-reel-video-renderer[is-active], ytd-reel-video-renderer[active], ytm-reel-item-renderer[is-active]') : null;
  const wrapper = document.querySelector('ytd-watch-flexy ytd-player') || document.querySelector('ytd-player');
  const players = short ? [active?.querySelector('.html5-video-player'), active?.querySelector('ytd-player')?.player_] : [document.getElementById('movie_player'), wrapper?.player_, wrapper?.player];
  let language = null;
  for (const player of players) {
   language = languageOf(safely(() => player?.getAudioTrack?.())) ||
    languageOf(safely(() => player?.getOption?.('audio', 'track')));
   if (language) break;
  }
  window.postMessage({type:'ldm-audio-track-response',requestId:event.data.requestId,language,
   videoId:new URL(location.href).pathname.match(/^\/shorts\/([^/]+)/)?.[1] || new URL(location.href).searchParams.get('v')}, location.origin);
 });
})();
