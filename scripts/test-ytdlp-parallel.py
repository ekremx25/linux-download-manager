import http.server, threading, tempfile, subprocess, time, json, shutil
from pathlib import Path
with tempfile.TemporaryDirectory(prefix='ldm-parallel-') as td:
 root=Path(td)
 subprocess.run(['ffmpeg','-y','-v','error','-f','lavfi','-i','color=c=blue:s=160x90:r=10','-t','1','-c:v','mpeg2video','-f','mpegts',str(root/'sample.ts')],check=True)
 fragment=(root/'sample.ts').read_bytes()
 stats={'active':0,'peak':0,'requests':0};lock=threading.Lock()
 class Handler(http.server.BaseHTTPRequestHandler):
  def log_message(self,*args):pass
  def do_GET(self):
   if self.path.startswith('/part-'):
    with lock:
     stats['active']+=1;stats['requests']+=1;stats['peak']=max(stats['peak'],stats['active'])
    try:
     time.sleep(.2)
     self.send_response(200);self.send_header('Content-Length',str(len(fragment)));self.end_headers();self.wfile.write(fragment)
    finally:
     with lock:stats['active']-=1
   else:
    data=('#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n'+''.join('#EXTINF:1,\npart-'+str(i)+'.ts\n' for i in range(30))+'#EXT-X-ENDLIST\n').encode()
    self.send_response(200);self.send_header('Content-Type','application/vnd.apple.mpegurl');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
 server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=server.serve_forever,daemon=True).start()
 proc=subprocess.run(['python3',str(Path(__file__).resolve().parents[1]/'src-tauri/src/download/ytdlp_parallel.py'),shutil.which('yt-dlp') or str(Path.home()/'.local/bin/yt-dlp'),'--no-playlist','--concurrent-fragments','10','--newline','--progress','--progress-template','download:LDM_PROGRESS:%(progress)j','-o',str(root/'result.mp4'),f'http://127.0.0.1:{server.server_port}/playlist.m3u8'],capture_output=True,text=True,timeout=60)
 server.shutdown()
 progress=[json.loads(line.split('LDM_PROGRESS:',1)[1]) for line in (proc.stdout+'\n'+proc.stderr).splitlines() if 'LDM_PROGRESS:{' in line]
 snapshots=[json.loads(line.split('LDM_PARTS:',1)[1]) for line in proc.stdout.splitlines() if line.startswith('LDM_PARTS:')]
 assert snapshots,proc.stdout[-1500:]+proc.stderr[-1500:]
 assert max(sum(row['status'] in ('receiving','connecting') for row in snap) for snap in snapshots)>=2
 assert any(row['status']=='connecting' and row['downloadedBytes']==0 for snap in snapshots for row in snap)
 assert max(len(snap) for snap in snapshots)<=10
 assert all(row['downloadedBytes']>=0 for snap in snapshots for row in snap)
 assert proc.returncode==0,proc.stderr[-1500:]
 assert stats['peak']==10,stats
 assert any(v.get('fragment_count')==30 and v.get('fragment_index')==30 for v in progress),progress[-1:]
 print('PASS:',json.dumps(stats),'30/30 fragments completed; real peak concurrency is 10.')
