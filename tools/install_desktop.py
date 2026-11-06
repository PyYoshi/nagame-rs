#!/usr/bin/env python3
"""Register Nagame's desktop entry and icons in the current user's XDG data directory."""
# SPDX-License-Identifier: GPL-3.0-or-later
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def exec_argument(path):
    # Desktop Entry Exec quoting is different from shell quoting. Escape the
    # command argument first, then the desktop string's backslash layer.
    value = str(path)
    if any(character in value for character in ('\n', '\r', '\t', '=')):
        raise ValueError('The executable path contains characters unsupported by desktop entries')
    value = ''.join('\\' + c if c in '\\"`$' else c for c in value)
    return '"' + value.replace('\\', '\\\\').replace('%', '%%') + '"'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path, help='The AppImage or standalone nagame executable to launch')
    arguments = parser.parse_args()
    binary = arguments.binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error('The binary must be an executable file')
    project = Path(__file__).resolve().parent.parent
    desktop = (project / 'packaging/linux/nagame.desktop').read_text()
    # GIO checks the first executable before expanding %% in its filename.
    # env keeps that first executable fixed and supports literal % in the path.
    desktop = desktop.replace('Exec=nagame %f', 'Exec=/usr/bin/env -- ' + exec_argument(binary) + ' %f')
    data = Path(os.environ.get('XDG_DATA_HOME') or Path.home() / '.local/share')
    if not data.is_absolute():
        parser.error('XDG_DATA_HOME must be an absolute path')
    applications = data / 'applications'
    applications.mkdir(parents=True, exist_ok=True)
    theme = data / 'icons/hicolor'
    for size in (32, 64, 256, 512):
        directory = theme / f'{size}x{size}/apps'
        directory.mkdir(parents=True, exist_ok=True)
        shutil.copy2(project / f'assets/branding/icon-{size}.png', directory / 'nagame.png')
    scalable = theme / 'scalable/apps'
    scalable.mkdir(parents=True, exist_ok=True)
    shutil.copy2(project / 'assets/branding/logo.svg', scalable / 'nagame.svg')
    # A running desktop can retain a failed theme-name lookup. An absolute
    # icon path also works independently of the active theme's search paths.
    icon = str(scalable / 'nagame.svg')
    icon = icon.replace('\\', '\\\\').replace('\n', '\\n').replace('\r', '\\r').replace('\t', '\\t')
    desktop = desktop.replace('Icon=nagame', 'Icon=' + icon)
    with tempfile.NamedTemporaryFile(mode='w', suffix='.desktop', dir=applications, delete=False) as stream:
        temporary = Path(stream.name)
        stream.write(desktop)
    try:
        validator = shutil.which('desktop-file-validate')
        if validator:
            subprocess.run([validator, str(temporary)], check=True)
        temporary.chmod(0o644)
        temporary.replace(applications / 'nagame.desktop')
    finally:
        temporary.unlink(missing_ok=True)
    for command, args in (
        ('gtk-update-icon-cache', ['-f', '-t', str(theme)]),
        ('update-desktop-database', [str(applications)]),
        ('kbuildsycoca6', []),
    ):
        executable = shutil.which(command)
        if executable:
            subprocess.run([executable, *args], check=True)
    notifier = shutil.which('dbus-send')
    if notifier and os.environ.get('DBUS_SESSION_BUS_ADDRESS'):
        # Same notification as KIconLoader::emitChange(Desktop); rebuild the
        # in-process KDE icon caches without restarting Plasma or its apps.
        subprocess.run([notifier, '--session', '--type=signal', '/KIconLoader',
                        'org.kde.KIconLoader.iconChanged', 'int32:0'], check=True)
    print('Registered:', applications / 'nagame.desktop')


if __name__ == '__main__':
    main()
