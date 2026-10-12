import ast,re,sys
from pathlib import Path
sys.path.insert(0,str(Path.home()/'.local/bin/yt-dlp'))
import yt_dlp
s=Path('src-tauri/src/download/ytdlp_parallel.py').read_text();tree=ast.parse(s)
ns={'yt_dlp':yt_dlp,'re':re,'original_selector':yt_dlp.YoutubeDL.build_format_selector}
f=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='format_selector');exec(compile(ast.Module(body=[f],type_ignores=[]),'<selector>','exec'),ns)
with yt_dlp.YoutubeDL({'quiet':True}) as y:
 select=ns['format_selector'](y,'bv+ba[language^=und]/b[language^=und]')
 formats=[{'format_id':'video','vcodec':'h264','acodec':'none','ext':'mp4','url':'https://example.com/v'}, {'format_id':'tr','vcodec':'none','acodec':'aac','language':'tr','ext':'m4a','url':'https://example.com/a'}]
 ctx={'formats':formats,'has_merged_format':False,'incomplete_formats':False}
 chosen=list(select(ctx));assert chosen[0]['format_id']=='video+tr',chosen
 formats.append({**formats[1],'format_id':'en','language':'en'})
 try:list(select(ctx));raise AssertionError('ambiguous languages accepted')
 except yt_dlp.utils.DownloadError:pass
print('PASS: unspecified player language resolves to sole available language; ambiguous languages rejected')
