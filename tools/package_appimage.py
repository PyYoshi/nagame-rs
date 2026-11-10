#!/usr/bin/env python3
"""PodmanまたはDockerで、AppImage・ライセンス表示・ソースアーカイブ・チェックサムをローカルに生成する。"""
# SPDX-License-Identifier: GPL-3.0-or-later
import hashlib
import html
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parent.parent


def run(*args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, text=True, **kwargs)


def capture(*args):
    return run(*args, stdout=subprocess.PIPE).stdout.strip()


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def container_engine():
    """justfileと同じ規則で選ぶ。指定がなければPodman、PodmanがなくDockerがあればDocker。"""
    if os.environ.get('CROSS_CONTAINER_ENGINE'):
        return os.environ['CROSS_CONTAINER_ENGINE']
    return 'docker' if not shutil.which('podman') and shutil.which('docker') else 'podman'


def mount(source, target, *, kind, writable=False):
    """コンテナーへ渡すバインドマウント。Dockerの -v は存在しないホストのパスを
    rootのディレクトリーとして作り、コンテナー内で実行できない空ディレクトリーになる。先に種類を確かめる。"""
    if not (source.is_file() if kind == 'file' else source.is_dir()):
        raise RuntimeError(f'Mount source is not a {kind}: {source}')
    return ['-v', f'{source}:{target}:{"" if writable else "ro,"}z']


def runs_as_root(engine):
    """コンテナー内のrootがホストのrootか。crossと同じく、Docker（Podmanの互換コマンドを除く）は
    rootで動き、ほかはrootlessとみなす。CROSS_ROOTLESS_CONTAINER_ENGINEで上書きできる。"""
    setting = os.environ.get('CROSS_ROOTLESS_CONTAINER_ENGINE', 'auto').lower()
    if setting in ('1', 'true'):
        return False
    if setting in ('0', 'false'):
        return True
    usage = run(engine, '--help', stdout=subprocess.PIPE, stderr=subprocess.STDOUT).stdout.lower()
    return 'docker' in usage and 'podman' not in usage and 'emulate' not in usage


def main():
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    engine = container_engine()
    as_root = runs_as_root(engine)
    output = ROOT / 'target/dist/appimage'
    output.mkdir(parents=True, exist_ok=True)
    binary = ROOT / 'target/dist-linux-x86_64/x86_64-unknown-linux-gnu/release/nagame'
    if not binary.is_file():
        raise RuntimeError(f'Distribution binary not found (run just dist-build): {binary}')
    # aqua whichはパスを返すだけで導入しない。チェックサムを照合して導入してから参照する。
    run('aqua', '-c', 'aqua.yaml', 'install')
    tools = {name: Path(capture('aqua', '-c', 'aqua.yaml', 'which', name))
             for name in ('linuxdeploy', 'appimagetool', 'appimage-runtime')}
    with tempfile.TemporaryDirectory(prefix='package-', dir=output) as temporary:
        work = Path(temporary)
        source = work / 'source'
        source.mkdir()
        licenses = work / 'licenses'
        licenses.mkdir()
        raw = work / 'about.json'
        run('aqua', '-c', 'aqua.yaml', 'exec', '--', 'cargo-about', 'generate', '--locked', '--fail',
            '--target', 'x86_64-unknown-linux-gnu', '--config', 'packaging/linux/about.toml',
            '--format', 'json', '-o', str(raw))
        data = json.loads(raw.read_text())
        notices = []
        for entry in data['licenses']:
            crates = [{key: used['crate'][key] for key in ('name', 'version', 'authors', 'repository')}
                      for used in entry['used_by']]
            notices.append(dict(name=entry['name'], id=entry['id'], text=entry['text'], crates=crates))
        (licenses / 'rust.json').write_text(json.dumps(notices, indent=2) + '\n')
        page = ['<!doctype html><meta charset="utf-8"><title>Nagame third-party licenses</title>',
                '<h1>Rust dependency licenses</h1>']
        for entry in notices:
            page += ['<h2>' + html.escape(entry['name']) + '</h2>',
                     '<p>' + html.escape(', '.join(c['name'] + ' ' + c['version'] for c in entry['crates'])) + '</p>',
                     '<pre>' + html.escape(entry['text']) + '</pre>']
        (licenses / 'rust.html').write_text('\n'.join(page))
        sysroot = Path(capture('rustup', 'run', '1.99.1', 'rustc', '--print', 'sysroot'))
        rust_docs = sysroot / 'share/doc/rust'
        shutil.copytree(rust_docs / 'licenses', licenses / 'rust-toolchain/licenses')
        shutil.copy2(rust_docs / 'COPYRIGHT-library.html', licenses / 'rust-toolchain/COPYRIGHT-library.html')

        # 追跡中のファイルと、無視されていない新規ファイルを写す。コミット前の変更も含む。
        project = source / 'nagame'
        project.mkdir()
        for name in capture('git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z').split('\0'):
            if not name:
                continue
            path = ROOT / name
            if path.is_file() or path.is_symlink():
                target = project / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, target, follow_symlinks=False)
        vendor = project / 'rust-vendor'
        config = capture('cargo', '+1.99.1', 'vendor', '--locked', '--versioned-dirs', str(vendor))
        config = config.replace(str(vendor), 'rust-vendor')
        (project / '.cargo').mkdir(exist_ok=True)
        # 既存の設定（PDFiumの環境変数）を残し、vendorの設定を追記する。
        existing = project / '.cargo/config.toml'
        prefix = existing.read_text() + '\n' if existing.exists() else ''
        existing.write_text(prefix + config + '\n')

        # ランタイムが含むLGPLのlibfuseには、ソースと再リンクの手順を添える。
        runtime_sources = source / 'appimage-runtime'
        runtime_sources.mkdir()
        runtime_licenses = licenses / 'appimage-runtime'
        runtime_licenses.mkdir()
        cache = ROOT / 'target/packaging-downloads'
        cache.mkdir(exist_ok=True)
        lock = json.loads((ROOT / 'packaging/linux/runtime-sources.json').read_text())
        for name, item in lock.items():
            path = cache / name
            if not path.exists() or digest(path) != item['sha256']:
                with urllib.request.urlopen(item['url'], timeout=120) as response:
                    path.write_bytes(response.read())
            if digest(path) != item['sha256']:
                raise RuntimeError('Source checksum mismatch: ' + name)
            shutil.copy2(path, runtime_sources / name)
            with tarfile.open(path) as archive:
                for member in archive.getmembers():
                    if Path(member.name).is_absolute() or '..' in Path(member.name).parts:
                        raise RuntimeError('Unsafe source archive member: ' + member.name)
                    if member.isfile() and Path(member.name).name.lower() in {
                        'license', 'license.txt', 'copying', 'copying.lib', 'copyright', 'lgpl2.txt', 'lgpl3.txt'}:
                        destination = runtime_licenses / name.removesuffix('.tar.gz').removesuffix('.tar.xz') / member.name
                        destination.parent.mkdir(parents=True, exist_ok=True)
                        with archive.extractfile(member) as stream:
                            destination.write_bytes(stream.read())
        shutil.copy2(ROOT / 'packaging/linux/runtime-sources.json', runtime_sources / 'sources.json')
        # PDFiumはUbuntuのパッケージではなく、SHA256で照合した固定版の配布物を同梱する。
        pdfium = Path(capture(sys.executable, str(ROOT / 'tools/fetch_pdfium.py'))).parent.parent
        mounts = [*mount(ROOT, '/source', kind='directory'),
                  *mount(work, '/work', kind='directory', writable=True),
                  *mount(binary.parent, '/binary', kind='directory'),
                  *mount(pdfium, '/pdfium', kind='directory')]
        for name, path in tools.items():
            mounts += mount(path, f'/tools/{name}', kind='file')
        image = 'localhost/nagame-package:ubuntu24.04'
        try:
            run(engine, 'run', '--rm', '--platform', 'linux/amd64', *mounts,
                image, 'python3', '/source/packaging/linux/package.py', version)
        finally:
            if as_root:
                # apt-get sourceのためにrootで組み立てる。失敗しても一時ディレクトリーを消せるよう、
                # 作ったファイルを利用者の所有に戻す。
                run(engine, 'run', '--rm', '--platform', 'linux/amd64',
                    *mount(work, '/work', kind='directory', writable=True), image, 'chown', '-R', f'{os.getuid()}:{os.getgid()}', '/work')
        shutil.copytree(work / 'Nagame.AppDir/usr/share/doc/nagame', source / 'licenses')
        shutil.copy2(ROOT / 'packaging/linux/SOURCE.md', source / 'README.md')
        archive_path = work / f'Nagame-{version}-source.tar.gz'
        with tarfile.open(archive_path, 'w:gz') as archive:
            def normalize(info):
                info.uid = info.gid = 0
                info.uname = info.gname = 'root'
                return info
            archive.add(source, arcname=f'nagame-{version}-source', filter=normalize)
        appimage = work / f'Nagame-{version}-x86_64.AppImage'
        build_info = dict(version=version, commit=capture('git', 'rev-parse', 'HEAD'),
                          dirty=bool(capture('git', 'status', '--porcelain')),
                          binary_sha256=digest(binary), appimage_sha256=digest(appimage),
                          runtime_sha256=digest(tools['appimage-runtime']),
                          pdfium=json.loads((ROOT / 'packaging/linux/pdfium.json').read_text()),
                          native=json.loads((work / 'native-manifest.json').read_text()))
        (work / 'build-info.json').write_text(json.dumps(build_info, indent=2) + '\n')
        artifacts = [appimage, archive_path, work / 'build-info.json']
        (work / 'SHA256SUMS').write_text(''.join(digest(p) + '  ' + p.name + '\n' for p in artifacts))
        for path in artifacts + [work / 'SHA256SUMS']:
            os.replace(path, output / path.name)
        print('Artifacts:', output.relative_to(ROOT))


if __name__ == '__main__':
    main()
