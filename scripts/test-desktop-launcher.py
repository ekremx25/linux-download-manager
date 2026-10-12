#!/usr/bin/env python3
"""Launch the packaged desktop entry through GLib, without installing anything.
Requires system Python with PyGObject; build the installer archive first.
"""
from pathlib import Path
import tarfile,tempfile,time,subprocess
from gi.repository import Gio
root=Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='ldm-launch-test-') as folder:
    target=Path(folder)/'Package with spaces';target.mkdir()
    archive=next((root/'dist').glob('LinuxDownloadManager-*.tar.gz'))
    with tarfile.open(archive) as tar:
        entry=next(member for member in tar.getmembers() if member.name.endswith('/Install.desktop'))
        (target/'Install.desktop').write_bytes(tar.extractfile(entry).read())
    # Only replace the payload: the real desktop entry must reach it correctly.
    (target/'Launch.sh').write_text('#!/bin/sh\nprintf success > "$(dirname "$0")/result"\n')
    subprocess.run(['desktop-file-validate',str(target/'Install.desktop')],check=True)
    app=Gio.DesktopAppInfo.new_from_filename(str(target/'Install.desktop'))
    assert app.launch([],None)
    result=target/'result'
    for _ in range(50):
        if result.exists():break
        time.sleep(.1)
    assert result.read_text()=='success'
    print('PASS: real GLib desktop launch reaches payload from a path containing spaces')
