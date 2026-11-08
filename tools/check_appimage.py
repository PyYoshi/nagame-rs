#!/usr/bin/env python3
"""AppImageを、隔離したUbuntuで（--hostなら現在のデスクトップで）起動して確かめる。"""
# SPDX-License-Identifier: GPL-3.0-or-later
import hashlib
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

from check_ui import png


def pdf(path):
    """赤い長方形を描いた1ページのPDFを書く（同梱したPDFiumでの描画の確認用）。"""
    content = b'1 0 0 rg 20 20 160 100 re f'
    objects = [b'<< /Type /Catalog /Pages 2 0 R >>',
               b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
               b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 140] /Contents 4 0 R >>',
               b'<< /Length %d >>\nstream\n%s\nendstream' % (len(content), content)]
    output = bytearray(b'%PDF-1.7\n')
    offsets = []
    for number, body in enumerate(objects, 1):
        offsets.append(len(output))
        output += b'%d 0 obj\n%s\nendobj\n' % (number, body)
    xref = len(output)
    output += b'xref\n0 %d\n0000000000 65535 f \n' % (len(objects) + 1)
    output += b''.join(b'%010d 00000 n \n' % offset for offset in offsets)
    output += b'trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n' % (len(objects) + 1, xref)
    path.write_bytes(bytes(output))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', action='store_true', help='現在のデスクトップとそのグラフィックドライバーを使う')
    host = parser.parse_args().host
    project = Path(__file__).resolve().parent.parent
    artifacts = project / 'target/dist/appimage' if host else Path('/artifacts')
    modes = []
    if host and os.environ.get('WAYLAND_DISPLAY'):
        modes.append('Wayland')
    if not host or os.environ.get('DISPLAY'):
        modes.append('X11')
    if not modes:
        raise RuntimeError('No Wayland or X11 display is available')
    info = json.loads((artifacts / 'build-info.json').read_text())
    image = artifacts / f'Nagame-{info["version"]}-x86_64.AppImage'
    for line in (artifacts / 'SHA256SUMS').read_text().splitlines():
        expected, name = line.split('  ', 1)
        with (artifacts / name).open('rb') as stream:
            assert hashlib.file_digest(stream, 'sha256').hexdigest() == expected, name
    with tempfile.TemporaryDirectory(prefix='nagame-appimage-check-') as temporary:
        root = Path(temporary)
        subprocess.run([str(image), '--appimage-extract'], cwd=root, check=True, stdout=subprocess.DEVNULL)
        app = root / 'squashfs-root'
        plugins = app / 'usr/lib/libheif'
        assert sorted(p.name for p in plugins.iterdir()) == ['libheif-aomdec.so', 'libheif-libde265.so']
        if not host:
            assert not Path('/usr/lib/x86_64-linux-gnu/libheif.so.1').exists()
        assert not list((app / 'usr/lib').glob('libvulkan*'))
        assert not list((app / 'usr/lib').glob('libwayland-*.so*')), 'Bundled Wayland can break newer host Vulkan drivers'
        assert not list((app / 'usr/lib').glob('libc.so*'))
        assert (app / 'usr/lib/libpdfium.so').is_file()
        assert (app / 'usr/share/doc/nagame/third-party/pdfium/LICENSE').is_file()
        subprocess.run([str(app / 'AppRun'), '--help'], check=True, stdout=subprocess.DEVNULL)
        subprocess.run([str(image), '--appimage-extract-and-run', '--help'], check=True, stdout=subprocess.DEVNULL)
        fixtures_directory = project / 'target/dist/codec-fixtures' if host else Path('/fixtures')
        fixtures = sorted(fixtures_directory.glob('*'))
        png(root / 'test.png', 320, 240, (220, 20, 10, 255))
        pdf(root / 'test.pdf')
        fixtures += [root / 'test.png', root / 'test.pdf']
        # 拡張子と中身が違うファイル（ダウンロードした画像に多い）も、中身の形式で開く。
        png(root / 'png-content.avif', 320, 240, (20, 220, 10, 255))
        shutil.copy(fixtures_directory / 'red-8.avif', root / 'avif-content.png')
        fixtures += [root / 'png-content.avif', root / 'avif-content.png']
        assert {p.suffix for p in fixtures} >= {'.avif', '.heic', '.png', '.pdf'}
        for mode, fixture in [(mode, fixture) for mode in modes for fixture in fixtures]:
            env = dict(os.environ, NAGAME_PROFILE='1', LANG='C.UTF-8', LANGUAGE='en',
                       XDG_CONFIG_HOME=str(root / 'config'))
            if not host:
                env['XDG_RUNTIME_DIR'] = str(root / 'runtime')
                Path(env['XDG_RUNTIME_DIR']).mkdir(mode=0o700, exist_ok=True)
            if mode == 'X11':
                env.pop('WAYLAND_DISPLAY', None)
            if host:
                env['VK_LOADER_DEBUG'] = 'error,warn'
            with (root / 'viewer.log').open('w+') as log:
                # ホストでは、利用者が実行するのと同じFUSE経由の入口から起動する。
                entry = image if host else app / 'AppRun'
                process = subprocess.Popen([str(entry), str(fixture)], env=env, stdout=log, stderr=log)
                try:
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        log.seek(0)
                        text = log.read()
                        if process.poll() is not None:
                            raise RuntimeError('Viewer exited: ' + text)
                        if re.search(r'load \d+ ms cached=\w+ animation=\w+', text):
                            time.sleep(0.5)
                            assert process.poll() is None, text
                            assert 'undefined symbol:' not in text, text
                            print(f'PASS: {mode} packaged GUI loaded {fixture.name}')
                            break
                        time.sleep(0.1)
                    else:
                        raise RuntimeError('No completed image load: ' + text)
                finally:
                    process.terminate()
                    process.wait(timeout=10)
        print('PASS: AppImage extraction, checksums, private plugins and ' + '/'.join(modes) + ' launch')


if __name__ == '__main__':
    main()
