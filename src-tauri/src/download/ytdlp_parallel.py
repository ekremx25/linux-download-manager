"""LDM's yt-dlp adapter: real fragment telemetry and validated HTTP byte ranges.
No URLs, cookies or headers are emitted in progress events.
"""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import threading
import time
import zipfile
from urllib.parse import urlsplit

executable = sys.argv.pop(1)
if not zipfile.is_zipfile(executable):
    os.execv(executable, [executable, *sys.argv[1:]])
sys.path.insert(0, executable)
import yt_dlp
from yt_dlp.downloader.http import HttpFD
from yt_dlp.networking import Request

lock = threading.RLock()
rows = []
slots = {}
last_emit = 0.0
original_hook = HttpFD._hook_progress
original_download = HttpFD.real_download


def emit(force=False):
    global last_emit
    now = time.monotonic()
    if force or now - last_emit >= .25:
        last_emit = now
        sys.stdout.write("LDM_PARTS:" + json.dumps(rows, separators=(',', ':')) + "\n")
        sys.stdout.flush()


def update(key, downloaded, total, status, label, start=0):
    with lock:
        if key not in slots:
            reusable = next((i for i, row in enumerate(rows) if row['status'] not in ('receiving', 'connecting')), None)
            if reusable is None:
                reusable = len(rows)
                rows.append({})
            else:
                for old, slot in list(slots.items()):
                    if slot == reusable:
                        del slots[old]
            slots[key] = reusable
        i = slots[key]
        rows[i] = dict(index=i, downloadedBytes=downloaded, totalBytes=total or 0,
                       status=status, label=label, start=start)
        emit(status not in ('receiving', 'connecting'))


def hook(self, status, info):
    filename = str(status.get('filename', ''))
    fragment = re.search(r'-Frag(\d+)$', filename)
    if fragment:
        update(filename, status.get('downloaded_bytes', 0), status.get('total_bytes'),
               'completed' if status.get('status') == 'finished' else ('receiving' if status.get('downloaded_bytes', 0) else 'connecting'),
               'Stream fragment ' + fragment.group(1))
    original_hook(self, status, info)


def reset_rows():
    with lock:
        rows.clear()
        slots.clear()
        emit(True)


def parallel_download(self, filename, info):
    # HLS/DASH fragments already use yt-dlp's bounded worker pool.
    fragment = re.search(r'-Frag(\d+)$', str(filename))
    if fragment:
        update(str(filename),0,0,'connecting','Stream fragment '+fragment.group(1))
        try:
            result = original_download(self, filename, info)
            if not result:
                update(str(filename),0,0,'failed','Stream fragment '+fragment.group(1))
            return result
        except Exception:
            update(str(filename),0,0,'failed','Stream fragment '+fragment.group(1))
            raise
    reset_rows()
    count = min(16, int(self.params.get('concurrent_fragment_downloads', 1)))
    if count <= 1 or info.get('request_data') or info.get('is_live') or filename == '-':
        return original_download(self, filename, info)
    headers = dict(info.get('http_headers') or {})

    def request(start, end):
        return self.ydl.urlopen(Request(info['url'], headers={
            **headers, 'Range': f'bytes={start}-{end}', 'Accept-Encoding': 'identity'},
            extensions={'timeout': 20}))

    # Do not split a server that ignores byte ranges or has an unknown size.
    try:
        with request(0, 0) as response:
            match = re.fullmatch(r'bytes 0-0/(\d+)', response.headers.get('Content-Range', ''))
            if response.status != 206 or not match:
                return original_download(self, filename, info)
            total = int(match[1])
            validator = response.headers.get('ETag', '') or response.headers.get('Last-Modified', '')
    except Exception:
        return original_download(self, filename, info)
    if total < 1024 * 1024 * count:
        return original_download(self, filename, info)

    # Respect extractor chunk limits. Google video throttles large individual
    # requests, even when several ranges are downloaded concurrently.
    chunk_limit = int(self.params.get('http_chunk_size') or
                      (info.get('downloader_options') or {}).get('http_chunk_size') or 0)
    host = (urlsplit(info['url']).hostname or '').lower()
    if host == 'googlevideo.com' or host.endswith('.googlevideo.com'):
        chunk_limit = min(chunk_limit or 8 * 1024 * 1024, 8 * 1024 * 1024)

    target = Path(filename)
    cache = Path(str(target) + '.ldm-ranges')
    identity = hashlib.sha256(json.dumps([info.get('id'), info.get('format_id'),
                                         total, count, validator]).encode()).hexdigest()
    if not (cache / 'identity').exists() or (cache / 'identity').read_text() != identity:
        shutil.rmtree(cache, ignore_errors=True)
    cache.mkdir(parents=True, exist_ok=True)
    (cache / 'identity').write_text(identity)
    stop = threading.Event()
    sizes = [0] * count
    ranges = [(i * total // count, (i + 1) * total // count - 1) for i in range(count)]
    began = time.monotonic()
    rate = self.params.get('ratelimit') or 0
    rate_lock = threading.Lock()
    due = [began]

    def worker(index):
        start, end = ranges[index]
        path = cache / str(index)
        expected = end - start + 1
        for attempt in range(3):
            done = path.stat().st_size if path.exists() else 0
            if done > expected:
                path.unlink()
                done = 0
            sizes[index] = done
            update(str(index), done, expected, 'completed' if done == expected else 'receiving',
                   f'HTTP part {index + 1}', start)
            if done == expected:
                return
            try:
                while done < expected and not stop.is_set():
                    request_start = start + done
                    request_end = min(end, request_start + chunk_limit - 1) if chunk_limit else end
                    request_size = request_end - request_start + 1
                    received = 0
                    with request(request_start, request_end) as response:
                        if response.status != 206 or response.headers.get('Content-Range') != f'bytes {request_start}-{request_end}/{total}':
                            raise ValueError('Server did not honor the requested range')
                        with path.open('ab') as output:
                            while not stop.is_set():
                                chunk = response.read(min(256 * 1024, request_size - received + 1))
                                if not chunk:
                                    break
                                if received + len(chunk) > request_size:
                                    raise ValueError('Oversized range response')
                                if rate:
                                    with rate_lock:
                                        due[0] = max(due[0], time.monotonic()) + len(chunk) / rate
                                        delay = due[0] - time.monotonic()
                                    if stop.wait(delay):
                                        return
                                output.write(chunk)
                                done += len(chunk)
                                received += len(chunk)
                                sizes[index] = done
                                update(str(index), done, expected, 'receiving', f'HTTP part {index + 1}', start)
                    if not stop.is_set() and received != request_size:
                        raise ValueError('Incomplete range response')
                if stop.is_set():
                    return
                if done != expected:
                    raise ValueError('Incomplete range response')
                update(str(index), done, expected, 'completed', f'HTTP part {index + 1}', start)
                return
            except Exception:
                if attempt == 2:
                    stop.set()
                    raise
                if stop.wait(.5 * (attempt + 1)):
                    return

    with lock:
        rows.clear()
        slots.clear()
        for i, (start, end) in enumerate(ranges):
            slots[str(i)] = i
            rows.append(dict(index=i,downloadedBytes=0,totalBytes=end-start+1,
                             status='queued',label=f'HTTP part {i+1}',start=start))
        emit(True)
    initial = sum((cache / str(i)).stat().st_size if (cache / str(i)).exists() else 0 for i in range(count))
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=count) as pool:
            futures = [pool.submit(worker, i) for i in range(count)]
            while not all(f.done() for f in futures):
                downloaded = sum(sizes)
                elapsed = max(.001, time.monotonic() - began)
                speed = max(0, downloaded - initial) / elapsed
                original_hook(self, dict(status='downloading', filename=filename,
                    downloaded_bytes=downloaded, total_bytes=total, speed=speed,
                    eta=(total-downloaded)/speed if speed else None, elapsed=elapsed), info)
                time.sleep(.2)
            for future in futures:
                future.result()
        temporary = Path(self.temp_name(filename))
        with temporary.open('wb') as output:
            for i in range(count):
                with (cache / str(i)).open('rb') as part:
                    shutil.copyfileobj(part, output, 1024 * 1024)
        if temporary.stat().st_size != total:
            raise ValueError('Merged media size mismatch')
        self.try_rename(str(temporary), filename)
        shutil.rmtree(cache)
        original_hook(self, dict(status='finished', filename=filename,
            downloaded_bytes=total, total_bytes=total, elapsed=time.monotonic()-began), info)
        return True
    except Exception:
        # A server can withdraw range support after the probe. Use yt-dlp's safe fallback.
        reset_rows()
        result = original_download(self, filename, info)
        if result:
            shutil.rmtree(cache, ignore_errors=True)
        return result


original_selector = yt_dlp.YoutubeDL.build_format_selector


def format_selector(self, spec):
    marker = '[language^=und]'
    if not isinstance(spec, str) or marker not in spec:
        return original_selector(self, spec)

    def select(context):
        audio = [f for f in context.get('formats', [])
                 if f.get('acodec') != 'none' and f.get('vcodec') == 'none']
        if not audio:
            audio = [f for f in context.get('formats', []) if f.get('acodec') != 'none']
        languages = {f.get('language') for f in audio}
        if len(languages) == 1:
            language = next(iter(languages))
            if language and language != 'und' and re.fullmatch(r'[a-zA-Z0-9-]+', language):
                return original_selector(self, spec.replace(marker, f'[language={language}]'))(context)
            if len(audio) == 1:
                return original_selector(self, spec.replace(marker, ''))(context)
        raise yt_dlp.utils.DownloadError('The selected audio language is unspecified and this video has multiple or unidentified audio tracks. Select an audio language in the player and try again.')
    return select


yt_dlp.YoutubeDL.build_format_selector = format_selector
HttpFD._hook_progress = hook
HttpFD.real_download = parallel_download
yt_dlp.main()
