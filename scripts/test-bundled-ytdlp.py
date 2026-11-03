#!/usr/bin/env python3
"""Offline tests for PATH resolution and AppImage helper installation."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class BundledHelperTests(unittest.TestCase):
    def test_adapter_resolves_bare_command_from_path(self):
        with tempfile.TemporaryDirectory() as td:
            helper = Path(td) / 'yt-dlp'
            helper.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
            helper.chmod(0o755)
            result = subprocess.run([sys.executable, str(ROOT / 'src-tauri/src/download/ytdlp_parallel.py'),
                                     'yt-dlp', '--version'], env=dict(os.environ, PATH=td),
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, '--version\n')

    def test_offline_install_and_preserve_user_helper(self):
        with tempfile.TemporaryDirectory() as td:
            home = Path(td) / 'home with spaces'
            home.mkdir()
            bundle = Path(td) / 'bundle'
            helper = bundle / 'usr/lib/ldm-tools/yt-dlp'
            helper.parent.mkdir(parents=True)
            helper.write_text('#!/bin/sh\necho bundled\n')
            helper.chmod(0o755)
            hook = ROOT / 'packaging/appimage/install-bundled-ytdlp.sh'
            cmd = ['bash', '-c', 'this_dir="$1"; source "$2"', 'test', str(bundle), str(hook)]
            env = dict(os.environ, HOME=str(home))
            first = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=10)
            self.assertEqual(first.returncode, 0, first.stderr)
            dest = home / '.local/bin/yt-dlp'
            self.assertEqual(dest.read_bytes(), helper.read_bytes())
            dest.write_text('#!/bin/sh\necho user\n')
            second = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=10)
            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertIn('echo user', dest.read_text())
            self.assertEqual(list(dest.parent.glob('*.install.*')), [])

    def test_broken_user_link_is_preserved_and_reported(self):
        with tempfile.TemporaryDirectory() as td:
            home = Path(td)
            dest = home / '.local/bin/yt-dlp'
            dest.parent.mkdir(parents=True)
            dest.symlink_to(home / 'missing')
            hook = ROOT / 'packaging/appimage/install-bundled-ytdlp.sh'
            result = subprocess.run(['bash', '-c', 'this_dir="$1"; source "$2"', 'test', td, str(hook)],
                                    env=dict(os.environ, HOME=td), capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue(dest.is_symlink())
            self.assertIn('not executable', result.stderr)

if __name__ == '__main__':
    unittest.main()
