#!/usr/bin/env python3
"""Exercise installer/update/uninstall safety using only temporary destinations."""
from pathlib import Path
import os, shutil, subprocess, tempfile, unittest
ROOT=Path(__file__).resolve().parents[1]

class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='ldm-installer-')
        self.root=Path(self.temp.name)
        self.package=self.root/'Paket boşluklu';self.package.mkdir()
        self.home=self.root/'Kullanıcı';self.home.mkdir()
        self.env=dict(os.environ,LDM_INSTALL_HOME=str(self.home),XDG_DATA_HOME=str(self.home/'data'))
        for name in ('install.sh','uninstall.sh'):shutil.copy2(ROOT/name,self.package/name)
        for name in ('bin','browser/chromium','src-tauri/icons'):(self.package/name).mkdir(parents=True,exist_ok=True)
        for name in ('linux-download-manager-custom','browser_native_host'):
            p=self.package/'bin'/name;p.write_text('#!/bin/sh\nexit 0\n');p.chmod(0o755)
        for name in ('manifest.json','service-worker.js','player-manifest-observer.js','content-script.js','content-style.css'):
            (self.package/'browser/chromium'/name).write_text('test')
        (self.package/'src-tauri/icons/icon.png').write_bytes(b'test')
    def tearDown(self):self.temp.cleanup()
    def run_install(self,*args):
        return subprocess.run(['bash',str(self.package/'install.sh'),'--no-deps',*args],env=self.env,text=True,capture_output=True)
    def test_check_makes_no_destination_changes(self):
        result=self.run_install('--check');self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(list(self.home.iterdir()),[])
    def test_missing_payload_keeps_existing_install(self):
        dest=self.home/'.local/bin/linux-download-manager';dest.parent.mkdir(parents=True);dest.write_text('old')
        (self.package/'bin/browser_native_host').unlink()
        result=self.run_install();self.assertNotEqual(result.returncode,0)
        self.assertEqual(dest.read_text(),'old')
    def test_update_backs_up_and_uninstall_keeps_history_and_config(self):
        data=self.home/'data/linux-download-manager-custom';data.mkdir(parents=True)
        history=data/'downloads.sqlite3';history.write_bytes(b'preserve history')
        config=self.home/'.config/yt-dlp/config';config.parent.mkdir(parents=True);config.write_text('--user-config-kept\n')
        dest=self.home/'.local/bin/linux-download-manager';dest.parent.mkdir(parents=True);dest.write_text('old binary')
        (self.home/'.config/microsoft-edge').mkdir()
        result=self.run_install();self.assertEqual(result.returncode,0,result.stderr)
        self.assertTrue(os.access(dest,os.X_OK))
        backups=list((data/'backups').iterdir());self.assertEqual(len(backups),1)
        self.assertEqual((backups[0]/str(dest).lstrip('/')).read_text(),'old binary')
        self.assertEqual(history.read_bytes(),b'preserve history');self.assertEqual(config.read_text(),'--user-config-kept\n')
        result=subprocess.run(['bash',str(self.package/'uninstall.sh')],env=self.env,capture_output=True)
        self.assertEqual(result.returncode,0,result.stderr);self.assertFalse(dest.exists())
        self.assertTrue(history.exists());self.assertTrue(config.exists());self.assertTrue(backups[0].exists())
    def test_corrupt_package_is_rejected_before_install(self):
        (self.package/'SHA256SUMS').write_text('0'*64+'  bin/linux-download-manager-custom\n')
        result=self.run_install();self.assertNotEqual(result.returncode,0)
        self.assertIn('Package integrity',result.stderr)
        self.assertEqual(list(self.home.iterdir()),[])
    def test_dirty_git_update_stops_before_fetch_or_install(self):
        shutil.rmtree(self.package/'bin');(self.package/'src-tauri/Cargo.toml').write_text('[package]\n')
        subprocess.run(['git','init','-q',str(self.package)],check=True)
        result=self.run_install('--update');self.assertNotEqual(result.returncode,0)
        self.assertIn('Uncommitted',result.stderr)
        self.assertEqual(list(self.home.iterdir()),[])

if __name__=='__main__':unittest.main()
