#!/usr/bin/env python3
"""UbuntuでAppDirを組み立て、同梱するネイティブパッケージのソースを集める。"""
# SPDX-License-Identifier: GPL-3.0-or-later
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def capture(*args):
    return run(*args, stdout=subprocess.PIPE).stdout.strip()


def main():
    source, work = Path('/source'), Path('/work')
    app = work / 'Nagame.AppDir'
    app.mkdir()
    (app / 'usr/bin').mkdir(parents=True)
    shutil.copy2('/binary/nagame', app / 'usr/bin/nagame')
    shutil.copy2(source / 'assets/branding/icon-256.png', work / 'nagame.png')
    tools = work / 'tools'
    tools.mkdir()
    for name in ('linuxdeploy', 'appimagetool'):
        directory = tools / name
        directory.mkdir()
        run('/tools/' + name, '--appimage-extract', cwd=directory, stdout=subprocess.DEVNULL)
    deploy = tools / 'linuxdeploy/squashfs-root/AppRun'
    libraries = [
        'libfontconfig.so.1', 'libfreetype.so.6',
        'libX11.so.6', 'libX11-xcb.so.1',
    ]
    ldconfig = capture('ldconfig', '-p')
    paths = {}
    for line in ldconfig.splitlines():
        if ' => ' in line:
            paths[line.split()[0]] = Path(line.split(' => ')[1])
    plugins = Path('/usr/lib/x86_64-linux-gnu/libheif/plugins')
    selected_plugins = [plugins / ('libheif-' + name + '.so') for name in ('aomdec', 'libde265')]
    command = [str(deploy), '--appdir', str(app), '--executable', str(app / 'usr/bin/nagame'),
               # ホストのVulkan ICDは、より新しいWaylandのシンボルを使うことがある。
               # Waylandのクライアントライブラリは同梱せず、ホストのグラフィックスタックと揃える。
               '--exclude-library', 'libwayland-*.so*',
               '--desktop-file', str(source / 'packaging/linux/nagame.desktop'),
               '--icon-file', str(work / 'nagame.png'),
               '--custom-apprun', str(source / 'packaging/linux/AppRun')]
    for path in [paths[name] for name in libraries] + selected_plugins:
        command += ['--library', str(path)]
    run(*command)
    # libheifはこの専用ディレクトリだけを探す。ホストのプラグインとエンコーダーは使わない。
    (app / 'usr/lib/libheif').mkdir()
    for plugin in selected_plugins:
        shutil.move(app / 'usr/lib' / plugin.name, app / 'usr/lib/libheif' / plugin.name)
    run('desktop-file-validate', str(app / 'nagame.desktop'))
    # PDFiumは固定版の配布物（packaging/linux/pdfium.json）。PDFの補助プロセスだけが、
    # 実行ファイルから見た ../lib/libpdfium.so を絶対パスで読み込む。
    shutil.copy2('/pdfium/lib/libpdfium.so', app / 'usr/lib/libpdfium.so')

    licenses = app / 'usr/share/doc/nagame'
    licenses.mkdir(parents=True)
    shutil.copy2(source / 'LICENSE', licenses / 'LICENSE')
    shutil.copytree(work / 'licenses', licenses / 'third-party')
    # PDFium本体と、PDFiumに組み込まれた第三者のライセンス。
    pdfium = licenses / 'third-party/pdfium'
    shutil.copytree('/pdfium/licenses', pdfium)
    shutil.copy2('/pdfium/LICENSE', pdfium / 'LICENSE')
    shutil.copy2('/pdfium/VERSION', pdfium / 'VERSION')
    shutil.copy2(source / 'packaging/linux/pdfium.json', pdfium / 'pdfium.json')
    native = licenses / 'third-party/native'
    native.mkdir()
    shutil.copytree('/usr/share/common-licenses', native / 'common-licenses')
    packages, files = {}, {}
    for library in (app / 'usr/lib').rglob('*'):
        if not library.is_file() or library.is_symlink() or library.name == 'libpdfium.so':
            continue
        # linuxdeployがstripやRPATHを変える前の、元のファイルから所有パッケージを探す。
        candidates = list(Path('/usr/lib/x86_64-linux-gnu').glob(library.name))
        candidates += list(plugins.glob(library.name))
        if not candidates:
            raise RuntimeError(f'No original library for {library.name}')
        original = candidates[0].resolve()
        owner = None
        for path in [str(original), str(original).replace('/usr/lib/', '/lib/', 1)]:
            result = subprocess.run(['dpkg-query', '-S', path], text=True, capture_output=True)
            if result.returncode == 0:
                owner = result.stdout.split(': ')[0]
                break
        if not owner:
            raise RuntimeError(f'No package owns {original}')
        record = capture('dpkg-query', '-W', '-f=${Package}\t${Version}\t${source:Package}\t${source:Version}', owner).split('\t')
        name, version, src, src_version = record
        packages[name] = dict(version=version, source=src, source_version=src_version)
        files[str(library.relative_to(app))] = name
        copyright_file = Path('/usr/share/doc') / name / 'copyright'
        shutil.copy2(copyright_file, native / (name + '.copyright'))
    manifest = dict(packages=packages, files=files)
    (native / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')

    # LD_LIBRARY_PATHを同梱ディレクトリにして、解決できないELFの依存がないことを確かめる
    # （システムの既定パスも探索される）。
    env = dict(os.environ, LD_LIBRARY_PATH=str(app / 'usr/lib'))
    for binary in [app / 'usr/bin/nagame'] + list((app / 'usr/lib').rglob('*.so*')):
        result = run('ldd', str(binary), env=env, stdout=subprocess.PIPE).stdout
        if 'not found' in result:
            raise RuntimeError(result)
    run(str(app / 'AppRun'), '--help')
    native_sources = work / 'source/native'
    native_sources.mkdir()
    run('apt-get', 'update')
    for src, version in sorted({(p['source'], p['source_version']) for p in packages.values()}):
        run('apt-get', 'source', '--download-only', '--only-source', src + '=' + version, cwd=native_sources)
    shutil.copy2(native / 'manifest.json', native_sources / 'manifest.json')

    version = sys.argv[1]
    output = work / f'Nagame-{version}-x86_64.AppImage'
    run(str(tools / 'appimagetool/squashfs-root/AppRun'), '--runtime-file', '/tools/appimage-runtime',
        '--no-appstream', str(app), str(output), env=dict(os.environ, ARCH='x86_64', VERSION=version))
    (work / 'native-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print('Created', output.name, hashlib.sha256(output.read_bytes()).hexdigest())


if __name__ == '__main__':
    main()
