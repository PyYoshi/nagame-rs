#!/usr/bin/env python3
"""Real GPUI window regression using app-dispatched keys, isolated settings and fixtures.

Build first: cargo build --locked --features ui-smoke
Run: python3 tools/check_ui.py [target/debug/nagame]
Requires a working desktop display. Does not inject desktop-wide input events.
"""
import os
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile
import zlib


def png(path, width, height, color):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    row = b'\0' + bytes(color) * width
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(row * height)) + chunk(b'IEND', b''))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', nargs='?', default='target/debug/nagame')
    parser.add_argument('--window-state-only', action='store_true')
    parser.add_argument('--wayland', action='store_true', help='Use the current Wayland session instead of X11')
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    with tempfile.TemporaryDirectory(prefix='nagame-window-check-') as temporary:
        root = Path(temporary)
        images = root / 'images'
        images.mkdir()
        source = root / 'source.png'
        png(source, 1600, 1200, (50, 80, 180, 255))
        for i in range(501):
            os.link(source, images / f'{i:04}.png')
        env = dict(os.environ, NAGAME_UI_SMOKE='1', RUST_BACKTRACE='1', XDG_CONFIG_HOME=str(root / 'config'))
        env.pop('NAGAME_UI_RESTART', None)
        env.pop('NAGAME_UI_STRESS', None)
        env.pop('NAGAME_UI_FOLDER_NAVIGATION', None)
        env.pop('NAGAME_UI_WINDOW_STATE', None)
        if args.wayland:
            if not (env.get('WAYLAND_DISPLAY') or env.get('WAYLAND_SOCKET')):
                raise RuntimeError('A Wayland session is required')
        else:
            env.pop('WAYLAND_DISPLAY', None)
            env.pop('WAYLAND_SOCKET', None)

        def run(path, extra):
            process = subprocess.Popen([str(binary), str(path)], env=dict(env, **extra))
            try:
                result = process.wait(timeout=40)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise RuntimeError('UI check timed out: build with --features ui-smoke') from None
            if result:
                raise RuntimeError(f'Viewer exited with status {result}')

        geometry_config = root / 'window-config'
        for mode in ('resize', 'restore', 'maximized'):
            run(source, {'NAGAME_UI_WINDOW_STATE': mode, 'XDG_CONFIG_HOME': str(geometry_config)})
        if args.window_state_only:
            print('PASS: window state save/restart/maximize/fullscreen regression')
            return
        folders = root / 'folders'
        for folder in ('01', '02'):
            directory = folders / folder
            directory.mkdir(parents=True)
            for name in ('01.png', '02.png', '03.png'):
                png(directory / name, 64, 64, (50, 80, 180, 255))
        run(folders / '02/01.png', {'NAGAME_UI_FOLDER_NAVIGATION': '1'})
        run(images / '0000.png', {'NAGAME_UI_STRESS': '1'})
        pages = root / 'pages'
        pages.mkdir()
        png(pages / '01.png', 400, 600, (255, 0, 0, 255))
        png(pages / '02.png', 400, 600, (0, 0, 255, 255))
        png(pages / '03.png', 800, 400, (0, 255, 0, 255))
        png(pages / '04.png', 400, 600, (255, 255, 0, 255))
        png(pages / '05.png', 400, 600, (180, 0, 180, 255))
        run(pages / '01.png', {})
        run(pages / '01.png', {'NAGAME_UI_RESTART': '1'})
        print('PASS: real-window regression and process shutdown')


if __name__ == '__main__':
    main()
