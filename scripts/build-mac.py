#!/usr/bin/env python3
"""Build an unsigned local macOS preview. No signing credentials are used."""
from pathlib import Path
import plistlib
import shutil
import subprocess
root = Path(__file__).resolve().parent.parent
subprocess.run(['swift', 'build', '-c', 'release', '--package-path', str(root/'native')], check=True)
binary = subprocess.check_output(['swift', 'build', '-c', 'release', '--show-bin-path', '--package-path', str(root/'native')], text=True).strip()
app = root/'dist'/'OmniuSong.app'
(app/'Contents'/'MacOS').mkdir(parents=True, exist_ok=True)
shutil.copy2(Path(binary)/'OmniuSongMac', app/'Contents'/'MacOS'/'OmniuSong')
info = {'CFBundleName':'OmniuSong','CFBundleDisplayName':'OmniuSong','CFBundleIdentifier':'cloud.omniusong.studio.dev','CFBundleExecutable':'OmniuSong','CFBundlePackageType':'APPL','CFBundleShortVersionString':'0.1.0','CFBundleVersion':'1','LSMinimumSystemVersion':'13.0','NSHighResolutionCapable':True,'NSMicrophoneUsageDescription':'Record your singing as an original vocal take in your local OmniuSong session.','NSAppTransportSecurity':{'NSAllowsLocalNetworking':True}}
with (app/'Contents'/'Info.plist').open('wb') as handle: plistlib.dump(info,handle)
print(app)
