#!/usr/bin/env python3
"""Exercise installer/update/uninstall safety using only temporary destinations."""
from pathlib import Path
import os, shutil, subprocess, tempfile, unittest, zipfile
ROOT=Path(__file__).resolve().parents[1]

class InstallerTests(unittest.TestCase):
    def setUp(self):
        base=Path(os.environ.get('LDM_INSTALLER_TEST_ROOT', tempfile.gettempdir()))
        base.mkdir(parents=True,exist_ok=True)
        self.temp=tempfile.TemporaryDirectory(prefix='ldm-installer-',dir=base)
        self.root=Path(self.temp.name)
        self.package=self.root/'Paket boşluklu';self.package.mkdir()
        self.home=self.root/'Kullanıcı';self.home.mkdir()
        self.tools=self.root/'tools';self.tools.mkdir()
        self.tmp=self.root/'tmp';self.tmp.mkdir()
        self.env=dict(LANG='C.UTF-8',HOME=str(self.home),LDM_INSTALL_HOME=str(self.home),
                      XDG_CONFIG_HOME=str(self.home/'.config'),XDG_DATA_HOME=str(self.home/'data'),
                      XDG_CACHE_HOME=str(self.home/'cache'),XDG_STATE_HOME=str(self.home/'state'),
                      XDG_RUNTIME_DIR=str(self.root/'runtime'),TMPDIR=str(self.tmp),
                      TMP=str(self.tmp),TEMP=str(self.tmp),PATH=f'{self.tools}:/usr/bin:/bin',
                      GIT_CONFIG_GLOBAL=os.devnull,GIT_CONFIG_SYSTEM=os.devnull,GIT_CONFIG_NOSYSTEM='1')
        # Firefox integration must not depend on a browser being installed on the host.
        (self.home/'.mozilla').mkdir()
        for name in ('curl','sudo','firefox','update-desktop-database','gtk-update-icon-cache'):
            tool=self.tools/name
            tool.write_text('#!/bin/sh\necho "unexpected external command" >&2\nexit 97\n')
            tool.chmod(0o755)
        self.initial_home=set(self.home.iterdir())
        for name in ('install.sh','uninstall.sh'):shutil.copy2(ROOT/name,self.package/name)
        for name in ('bin','browser/chromium','browser/firefox','src-tauri/icons'):(self.package/name).mkdir(parents=True,exist_ok=True)
        for name in ('linux-download-manager','browser_native_host'):
            p=self.package/'bin'/name;p.write_text('#!/bin/sh\nexit 0\n');p.chmod(0o755)
        app=self.package/'bin/linux-download-manager'
        app.write_text("#!/bin/sh\nif [ \"$1\" = --package-firefox-extension ]; then\n  python3 - \"$2\" \"$3\" <<'PY'\nfrom pathlib import Path\nfrom zipfile import ZipFile\nimport sys\nsource=Path(sys.argv[1]);out=Path(sys.argv[2]);out.parent.mkdir(parents=True,exist_ok=True)\nwith ZipFile(out,'w') as archive:\n for file in source.iterdir():archive.write(file,file.name)\nPY\nfi\n")
        for name in ('manifest.json','service-worker.js','player-manifest-observer.js','youtube-audio-observer.js','content-script.js','content-style.css','popup.html','popup.js','popup.css'):
            (self.package/'browser/chromium'/name).write_text('test')
        (self.package/'browser/firefox/manifest.json').write_text('test')
        (self.package/'src-tauri/icons/icon.png').write_bytes(b'test')
    def tearDown(self):self.temp.cleanup()
    def run_install(self,*args,deps=False):
        options=[] if deps else ['--no-deps']
        return subprocess.run(['bash',str(self.package/'install.sh'),*options,*args],env=self.env,text=True,capture_output=True,timeout=30)
    def test_check_makes_no_destination_changes(self):
        result=self.run_install('--check');self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(set(self.home.iterdir()),self.initial_home)
    def test_missing_payload_keeps_existing_install(self):
        dest=self.home/'.local/bin/linux-download-manager';dest.parent.mkdir(parents=True);dest.write_text('old')
        (self.package/'bin/browser_native_host').unlink()
        result=self.run_install();self.assertNotEqual(result.returncode,0)
        self.assertEqual(dest.read_text(),'old')
    def test_update_leaves_no_backups_and_uninstall_keeps_history_and_config(self):
        data=self.home/'data/linux-download-manager';data.mkdir(parents=True)
        history=data/'downloads.sqlite3';history.write_bytes(b'preserve history')
        config=self.home/'.config/yt-dlp/config';config.parent.mkdir(parents=True);config.write_text('--user-config-kept\n')
        dest=self.home/'.local/bin/linux-download-manager';dest.parent.mkdir(parents=True);dest.write_text('old binary')
        (self.home/'.config/microsoft-edge').mkdir()
        result=self.run_install();self.assertEqual(result.returncode,0,result.stderr)
        self.assertTrue(os.access(dest,os.X_OK))
        self.assertTrue((self.home/"Documents/Linux Download Manager Extension/popup.html").exists())
        self.assertTrue((self.home/"Documents/Linux Download Manager Firefox Extension/popup.html").exists())
        firefox_zip=self.home/'Documents/Linux Download Manager Firefox Extension.zip'
        with zipfile.ZipFile(firefox_zip) as archive:
            self.assertIn('manifest.json',archive.namelist())
        self.assertFalse((self.home/'Documents/Linux Download Manager Extensions').exists())
        firefox_host=self.home/'.mozilla/native-messaging-hosts/com.eko.linuxdownloadmanager.json'
        self.assertIn('ldm-bridge@ekremx25.github.io',firefox_host.read_text())
        self.assertFalse((data/'backups').exists())
        self.assertEqual(history.read_bytes(),b'preserve history');self.assertEqual(config.read_text(),'--user-config-kept\n')
        result=subprocess.run(['bash',str(self.package/'uninstall.sh')],env=self.env,capture_output=True,timeout=30)
        self.assertEqual(result.returncode,0,result.stderr);self.assertFalse(dest.exists())
        self.assertTrue(history.exists());self.assertTrue(config.exists())
        self.assertFalse(firefox_host.exists())
        self.assertFalse(firefox_zip.exists())
    def test_legacy_custom_data_folder_is_adopted(self):
        legacy=self.home/'data/linux-download-manager-custom';legacy.mkdir(parents=True)
        (legacy/'downloads.sqlite3').write_bytes(b'legacy history')
        result=self.run_install();self.assertEqual(result.returncode,0,result.stderr)
        self.assertFalse(legacy.exists())
        self.assertEqual((self.home/'data/linux-download-manager/downloads.sqlite3').read_bytes(),b'legacy history')
    def snapshot_home(self):
        snapshot={}
        for path in self.home.rglob('*'):
            relative=path.relative_to(self.home)
            if 'backups' in relative.parts:
                continue
            if path.is_symlink():
                snapshot[str(relative)]=('link',os.readlink(path))
            elif path.is_file():
                snapshot[str(relative)]=(path.read_bytes(),path.stat().st_mode & 0o777)
        return snapshot
    def seed_previous_install(self):
        (self.home/'.config/chromium').mkdir(parents=True)
        result=self.run_install();self.assertEqual(result.returncode,0,result.stderr)
        for relative in ('data/linux-download-manager/downloads.sqlite3',
                         '.config/yt-dlp/config','Documents/Linux Download Manager Extension/user-notes.txt',
                         'Documents/Linux Download Manager Firefox Extension/user-notes.txt',
                         'Documents/unrelated.txt'):
            path=self.home/relative;path.parent.mkdir(parents=True,exist_ok=True)
            path.write_text('user data must survive: '+relative)
        # Preserve metadata as well as bytes, including an existing symlink.
        dest=self.home/'.local/bin/linux-download-manager'
        dest.unlink();dest.symlink_to(self.package/'bin/linux-download-manager')
        (self.home/'data/linux-download-manager/.setup_done').chmod(0o600)
        return self.snapshot_home()
    def test_failed_firefox_packaging_restores_previous_install(self):
        before=self.seed_previous_install()
        app=self.package/'bin/linux-download-manager'
        app.write_text('#!/bin/sh\nprintf broken > "$3"\nprintf partial > "$3.tmp"\nexit 42\n')
        result=self.run_install();self.assertNotEqual(result.returncode,0)
        self.assertIn('Firefox extension ZIP',result.stderr)
        self.assertEqual(self.snapshot_home(),before)
        self.assertFalse(list(self.home.rglob('*.new.*')))
    def test_failed_atomic_replacements_restore_every_phase(self):
        before=self.seed_previous_install()
        mv=self.tools/'mv'
        mv.write_text('#!/bin/bash\ndestination="${!#}"\n'
                      'if [[ "$destination" == "$FAIL_DEST" && ! -e "$FAIL_ONCE" ]]; then\n'
                      '  : > "$FAIL_ONCE"; exit 43\nfi\nexec /usr/bin/mv "$@"\n')
        mv.chmod(0o755)
        for relative in ('.local/bin/linux-download-manager',
                         'data/linux-download-manager/bin/browser_native_host',
                         'Documents/Linux Download Manager Extension/popup.js',
                         'Documents/Linux Download Manager Firefox Extension.zip',
                         'data/icons/hicolor/256x256/apps/linux-download-manager.png',
                         'data/applications/linux-download-manager.desktop',
                         '.config/chromium/NativeMessagingHosts/com.eko.linuxdownloadmanager.json',
                         '.mozilla/native-messaging-hosts/com.eko.linuxdownloadmanager.json',
                         'data/linux-download-manager/.setup_done',
                         'data/linux-download-manager/uninstall.sh'):
            with self.subTest(destination=relative):
                once=self.tmp/'failed-once';once.unlink(missing_ok=True)
                self.env.update(FAIL_DEST=str(self.home/relative),FAIL_ONCE=str(once))
                result=self.run_install();self.assertNotEqual(result.returncode,0)
                self.assertTrue(once.exists(),'fixture must reach the intended replacement')
                self.assertEqual(self.snapshot_home(),before)
                self.assertFalse(list(self.home.rglob('*.new.*')))
    def test_failed_fresh_install_removes_only_new_owned_files(self):
        unrelated=self.home/'Documents/Linux Download Manager Extension/user-notes.txt'
        unrelated.parent.mkdir(parents=True);unrelated.write_text('keep')
        before=self.snapshot_home()
        app=self.package/'bin/linux-download-manager'
        app.write_text('#!/bin/sh\nprintf broken > "$3"\nprintf partial > "$3.tmp"\nexit 42\n')
        result=self.run_install();self.assertNotEqual(result.returncode,0)
        self.assertIn('Firefox extension ZIP',result.stderr)
        self.assertEqual(self.snapshot_home(),before)
    def mock_helper_download(self,mode):
        # Exclude system/user yt-dlp and package managers completely.
        for name in ('bash','sh','python3','uname','date','mktemp','mkdir','dirname','basename',
                     'cp','install','mv','chmod','rm','cat','grep','sed','sha256sum'):
            (self.tools/name).symlink_to(shutil.which(name))
        for name in ('ldd','ffmpeg','node'):
            path=self.tools/name;path.write_text('#!/bin/sh\nexit 0\n');path.chmod(0o755)
        self.env.update(PATH=str(self.tools),DOWNLOAD_MODE=mode,
                        CURL_TRACE=str(self.tmp/'curl.jsonl'),HELPER_EXECUTED=str(self.tmp/'executed'))
        curl=self.tools/'curl'
        curl.write_text('''#!/usr/bin/python3
import hashlib,json,os,sys
from pathlib import Path
args=sys.argv[1:]
with open(os.environ['CURL_TRACE'],'a') as trace: trace.write(json.dumps(args)+'\\n')
for option in ('--fail','--location','--connect-timeout','--max-time','--retry-max-time','--max-filesize'):
    assert option in args,option
assert args[args.index('--proto')+1]=='=https'
assert args[args.index('--proto-redir')+1]=='=https'
out=Path(args[args.index('--output')+1]);url=args[-1]
payload=b'#!/bin/sh\\n: > "$HELPER_EXECUTED"\\n'
mode=os.environ['DOWNLOAD_MODE']
if url.endswith('/SHA2-256SUMS'):
    requests=[json.loads(line) for line in Path(os.environ['CURL_TRACE']).read_text().splitlines()]
    binary=requests[-2]
    staged=Path(binary[binary.index('--output')+1])
    assert not (staged.stat().st_mode & 0o111),'checksum must precede chmod'
    assert not os.access(Path(os.environ['HOME'])/'.local/bin/yt-dlp',os.X_OK),'checksum must precede publication'
    if mode=='checksum-fail': sys.exit(22)
    digest='0'*64 if mode=='mismatch' else hashlib.sha256(payload).hexdigest()
    out.write_text(digest+'  yt-dlp\\n')
else:
    out.write_bytes(b'' if mode=='empty' else payload)
    assert not (out.stat().st_mode & 0o111),'download must not be executable yet'
    if mode=='partial': sys.exit(18)
''')
        curl.chmod(0o755)
    def test_helper_download_failures_do_not_publish_or_execute(self):
        self.mock_helper_download('partial')
        for mode in ('partial','mismatch','empty','checksum-fail'):
            with self.subTest(mode=mode):
                # A previous non-executable helper must also survive failed replacement.
                helper=self.home/'.local/bin/yt-dlp';helper.parent.mkdir(parents=True,exist_ok=True)
                helper.write_bytes(b'old incomplete helper');helper.chmod(0o600)
                before=self.snapshot_home();self.env['DOWNLOAD_MODE']=mode
                result=self.run_install(deps=True);self.assertNotEqual(result.returncode,0)
                self.assertIn('yt-dlp',result.stderr)
                self.assertEqual(self.snapshot_home(),before)
                self.assertFalse((self.tmp/'executed').exists())
                self.assertFalse(list(self.home.rglob('*.new.*')))
    def test_verified_helper_is_published_without_running_downloaded_bytes(self):
        self.mock_helper_download('valid')
        result=self.run_install(deps=True);self.assertEqual(result.returncode,0,result.stderr)
        helper=self.home/'.local/bin/yt-dlp'
        self.assertEqual(helper.read_bytes(),b'#!/bin/sh\n: > "$HELPER_EXECUTED"\n')
        self.assertEqual(helper.stat().st_mode & 0o777,0o755)
        self.assertFalse((self.tmp/'executed').exists())
        self.assertIn('upstream HTTPS SHA256',result.stdout)
        self.assertFalse((self.home/'.config/yt-dlp/config').exists())
        self.assertFalse(list(self.home.rglob('*.new.*')))
    def test_helper_install_preserves_shared_ytdlp_config(self):
        self.mock_helper_download('valid')
        config=self.home/'.config/yt-dlp/config'
        config.parent.mkdir(parents=True,exist_ok=True)
        original=b'--js-runtimes deno:/custom/runtime\n--mtime\n'
        config.write_bytes(original)
        result=self.run_install(deps=True);self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(config.read_bytes(),original)
    def test_xdg_config_native_hosts_are_installed_and_removed(self):
        config=self.home/'custom config';(config/'chromium').mkdir(parents=True)
        self.env['XDG_CONFIG_HOME']=str(config)
        result=self.run_install();self.assertEqual(result.returncode,0,result.stderr)
        host=config/'chromium/NativeMessagingHosts/com.eko.linuxdownloadmanager.json'
        self.assertTrue(host.exists())
        self.assertFalse((self.home/'.config/chromium').exists())
        result=subprocess.run(['bash',str(self.package/'uninstall.sh')],env=self.env,capture_output=True,timeout=30)
        self.assertEqual(result.returncode,0,result.stderr);self.assertFalse(host.exists())
    def test_corrupt_package_is_rejected_before_install(self):
        (self.package/'SHA256SUMS').write_text('0'*64+'  bin/linux-download-manager\n')
        result=self.run_install();self.assertNotEqual(result.returncode,0)
        self.assertIn('Package integrity',result.stderr)
        self.assertEqual(set(self.home.iterdir()),self.initial_home)
    def test_dirty_git_update_stops_before_fetch_or_install(self):
        shutil.rmtree(self.package/'bin');(self.package/'src-tauri/Cargo.toml').write_text('[package]\n')
        subprocess.run(['git','init','-q',str(self.package)],env=self.env,check=True,timeout=10)
        result=self.run_install('--update');self.assertNotEqual(result.returncode,0)
        self.assertIn('Uncommitted',result.stderr)
        self.assertEqual(set(self.home.iterdir()),self.initial_home)

if __name__=='__main__':unittest.main()
