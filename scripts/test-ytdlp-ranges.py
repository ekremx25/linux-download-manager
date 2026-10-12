"""Actual adapter test: ten concurrent HTTP ranges, exact bytes and range rejection."""
import hashlib
import http.server
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import threading
import time

adapter=Path(__file__).resolve().parents[1]/'src-tauri/src/download/ytdlp_parallel.py'
executable=shutil.which('yt-dlp') or str(Path.home()/'.local/bin/yt-dlp')
payload=bytes(range(256))*81920 # 20 MiB, distinctive bytes for range/merge validation
stats={'active':0,'peak':0,'ranges':0,'oversized':0,'capped':0}
guard=threading.Lock()
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_HEAD(self):
  self.send_response(200);self.send_header('Content-Type','video/mp4');self.send_header('Content-Length',str(len(payload)));self.end_headers()
 def do_GET(self):
  ranged=self.headers.get('Range')
  match=re.fullmatch(r'bytes=(\d+)-(\d+)',ranged or '')
  start,end=(int(match[1]),int(match[2])) if match else (0,len(payload)-1)
  if self.path.startswith('/capped') and match and end>0:
   stats['capped']+=1
   if end-start+1>1024*1024:
    stats['oversized']+=1;self.send_error(429);return
  if self.path.startswith('/reject') and match and end>0:
   start,end=0,len(payload)-1;match=None
  with guard:
   stats['active']+=1;stats['peak']=max(stats['peak'],stats['active'])
   if match and end>0:stats['ranges']+=1
  try:
   self.send_response(206 if match else 200)
   self.send_header('Content-Type','video/mp4')
   if match:self.send_header('Content-Range',f'bytes {start}-{end}/{len(payload)}')
   self.send_header('Content-Length',str(end-start+1));self.end_headers()
   if end>0:time.sleep(.15)
   for offset in range(start,end+1,65536):
    self.wfile.write(payload[offset:min(offset+65536,end+1)])
    if end>0:time.sleep(.003)
  except (BrokenPipeError,ConnectionResetError):pass
  finally:
   with guard:stats['active']-=1

server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=server.serve_forever,daemon=True).start()
with tempfile.TemporaryDirectory(prefix='ldm-ranges-test-') as td:
 root=Path(td)
 def command(name):
  return ['python3',str(adapter),executable,'--no-playlist','--concurrent-fragments','10','--newline','--progress','-o',str(root/(name+'.mp4')),f'http://127.0.0.1:{server.server_port}/{name}.mp4']
 result=subprocess.run(command('range'),capture_output=True,text=True,timeout=60)
 assert result.returncode==0,result.stderr[-2000:]
 assert (root/'range.mp4').read_bytes()==payload
 snapshots=[json.loads(l.split(':',1)[1]) for l in result.stdout.splitlines() if l.startswith('LDM_PARTS:')]
 assert any(len(s)==10 and all(p['status']=='completed' for p in s) for s in snapshots),result.stdout[-2000:]
 assert stats['ranges']==10,stats
 assert stats['peak']>=10,stats
 print('PASS: 10 real ranges, 10 telemetry rows, SHA-256',hashlib.sha256(payload).hexdigest())
 capped=command('capped');capped[3:3]=['--http-chunk-size','1M']
 result=subprocess.run(capped,capture_output=True,text=True,timeout=60)
 assert result.returncode==0,result.stderr[-2000:]
 assert (root/'capped.mp4').read_bytes()==payload
 assert stats['oversized']==0 and stats['capped']>=20,stats
 print('PASS: bounded HTTP requests respect chunk limit and merge exact bytes')
 # Interrupt an actual process and resume using saved parts; no stale data/duplicates.
 proc=subprocess.Popen(command('resume'),stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 cache=root/'resume.mp4.ldm-ranges'
 deadline=time.monotonic()+15
 while time.monotonic()<deadline:
  if cache.exists() and sum(p.stat().st_size for p in cache.iterdir() if p.name.isdigit())>1024*1024:break
  time.sleep(.02)
 assert proc.poll() is None
 proc.kill();proc.wait()
 assert cache.exists()
 result=subprocess.run(command('resume'),capture_output=True,text=True,timeout=60)
 assert result.returncode==0,result.stderr[-2000:]
 assert (root/'resume.mp4').read_bytes()==payload
 print('PASS: interrupted range transfer resumes and merges exact bytes')
 result=subprocess.run(command('reject'),capture_output=True,text=True,timeout=60)
 assert result.returncode==0,result.stderr[-2000:]
 assert (root/'reject.mp4').read_bytes()==payload
 print('PASS: server withdrawing range support falls back without corrupting output')
server.shutdown()
