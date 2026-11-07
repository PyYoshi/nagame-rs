#!/usr/bin/env python3
"""固定版のPDFium（bblanchon/pdfium-binaries）を取得し、SHA256を照合してtarget/pdfiumへ展開する。

版・URL・SHA256はpackaging/linux/pdfium.jsonで固定する。展開済みで照合値が同じなら何もしない。
ghがあれば出所の証明（GitHubのattestation）も検証する。版を更新するときは--require-attestationで
証明の検証を必須にする。最後にlibpdfium.soの絶対パスを出力する。
"""
# SPDX-License-Identifier: GPL-3.0-or-later
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / 'packaging/linux/pdfium.json'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--require-attestation', action='store_true',
                        help='ghによる出所の証明の検証を必須にする（版の更新時）')
    arguments = parser.parse_args()
    manifest = json.loads(MANIFEST.read_text())
    destination = ROOT / 'target/pdfium' / manifest['tag'].replace('/', '-')
    library = destination / 'lib/libpdfium.so'
    stamp = destination / '.sha256'
    if library.is_file() and stamp.is_file() and stamp.read_text().strip() == manifest['sha256']:
        print(library)
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='fetch-', dir=destination.parent) as temporary:
        work = Path(temporary)
        archive = work / manifest['archive']
        with urllib.request.urlopen(manifest['url']) as response, archive.open('wb') as stream:
            shutil.copyfileobj(response, stream)
        actual = digest(archive)
        if actual != manifest['sha256']:
            sys.exit(f'SHA256 mismatch for {manifest["archive"]}: {actual}')
        gh = shutil.which('gh')
        if gh:
            subprocess.run([gh, 'attestation', 'verify', str(archive),
                            '--repo', 'bblanchon/pdfium-binaries'], check=True)
        elif arguments.require_attestation:
            sys.exit('gh is required to verify the attestation')
        else:
            print('gh not found: verified the pinned SHA256 only', file=sys.stderr)
        extracted = work / 'pdfium'
        with tarfile.open(archive) as stream:
            stream.extractall(extracted, filter='data')
        if not (extracted / 'lib/libpdfium.so').is_file():
            sys.exit('The archive has no lib/libpdfium.so')
        (extracted / '.sha256').write_text(manifest['sha256'] + '\n')
        if destination.exists():
            shutil.rmtree(destination)
        extracted.rename(destination)
    print(library)


if __name__ == '__main__':
    main()
