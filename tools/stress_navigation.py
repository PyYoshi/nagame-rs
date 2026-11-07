#!/usr/bin/env python3
"""X11で、高速な画像送りによるテクスチャの退役を確かめるGUI回帰テスト。

libX11・libXtst・xpropが必要。生成したPNGで隔離したビューアーを起動し、そのウィンドウだけを操作対象にする。
使い方: python3 tools/stress_navigation.py [path/to/nagame]
"""
import ctypes as c
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import time
import zlib


def png(path):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    width, height = 1600, 1200
    row = b'\0' + b''.join(bytes((x % 256, 80, 180, 255)) for x in range(width))
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0))
                    + chunk(b'IDAT', zlib.compress(row * height)) + chunk(b'IEND', b''))


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/nagame').resolve()
    x = c.CDLL('libX11.so.6')
    t = c.CDLL('libXtst.so.6')
    x.XOpenDisplay.argtypes = [c.c_char_p]
    x.XOpenDisplay.restype = c.c_void_p
    x.XStringToKeysym.argtypes = [c.c_char_p]
    x.XStringToKeysym.restype = c.c_ulong
    x.XKeysymToKeycode.argtypes = [c.c_void_p, c.c_ulong]
    x.XKeysymToKeycode.restype = c.c_uint
    x.XSetInputFocus.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_ulong]
    x.XFlush.argtypes = [c.c_void_p]
    x.XCloseDisplay.argtypes = [c.c_void_p]
    t.XTestFakeKeyEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
    display = x.XOpenDisplay(None)
    if not display:
        raise RuntimeError('X11 display connection required')
    try:
        with tempfile.TemporaryDirectory(prefix='nagame-atlas-stress-') as temporary:
            root = Path(temporary)
            folder = root / 'images'
            folder.mkdir()
            source = root / 'source.png'
            png(source)
            prefix = f'nagame-stress-{os.getpid()}-'
            for i in range(501):
                os.link(source, folder / f'{prefix}{i:04}.png')
            env = os.environ.copy()
            env.pop('WAYLAND_DISPLAY', None)
            env['RUST_BACKTRACE'] = '1'
            env['XDG_CONFIG_HOME'] = str(root / 'config')
            with (root / 'viewer.log').open('w+') as log:
                process = subprocess.Popen([str(binary), str(folder / f'{prefix}0000.png')], env=env, stdout=log, stderr=log)
                try:
                    def check_alive():
                        if process.poll() is not None:
                            log.seek(0)
                            raise RuntimeError('Viewer terminated:\n' + log.read())

                    def title(window):
                        return subprocess.check_output(['xprop', '-id', window, '_NET_WM_NAME'], text=True)

                    window = None
                    deadline = time.monotonic() + 15
                    while time.monotonic() < deadline and window is None:
                        check_alive()
                        clients = subprocess.check_output(['xprop', '-root', '_NET_CLIENT_LIST'], text=True)
                        for candidate in re.findall(r'0x[0-9a-f]+', clients):
                            if prefix in title(candidate):
                                window = candidate
                                break
                        time.sleep(.05)
                    if window is None:
                        raise RuntimeError('Viewer window did not appear')
                    x.XSetInputFocus(display, int(window, 16), 1, 0)
                    x.XFlush(display)
                    time.sleep(.2)
                    previous = 0
                    for direction in [b'Right', b'Left', b'Right', b'Left']:
                        x.XSetInputFocus(display, int(window, 16), 1, 0)
                        x.XFlush(display)
                        code = x.XKeysymToKeycode(display, x.XStringToKeysym(direction))
                        for _ in range(350):
                            check_alive()
                            t.XTestFakeKeyEvent(display, code, 1, 0)
                            t.XTestFakeKeyEvent(display, code, 0, 0)
                            x.XFlush(display)
                            time.sleep(.008)
                        time.sleep(.4)
                        check_alive()
                        current = title(window)
                        index = int(re.search(re.escape(prefix) + r'(\d{4})\.png', current).group(1))
                        assert (index > previous if direction == b'Right' else index < previous), current
                        previous = index
                        print(f'{direction.decode()}: 350 input events; final image {index}; viewer alive', flush=True)
                    # process.terminate()で終了時のエラーを隠さず、実際にウィンドウを閉じる
                    # 処理も通す。
                    class ClientMessage(c.Structure):
                        _fields_ = [('type', c.c_int), ('serial', c.c_ulong), ('send_event', c.c_int),
                                    ('display', c.c_void_p), ('window', c.c_ulong),
                                    ('message_type', c.c_ulong), ('format', c.c_int), ('data', c.c_long * 5)]
                    class Event(c.Union):
                        _fields_ = [('client', ClientMessage), ('pad', c.c_long * 24)]
                    x.XInternAtom.argtypes = [c.c_void_p, c.c_char_p, c.c_int]
                    x.XInternAtom.restype = c.c_ulong
                    x.XSendEvent.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_long, c.POINTER(Event)]
                    event = Event()
                    event.client = ClientMessage(type=33, send_event=1, display=display,
                        window=int(window, 16), message_type=x.XInternAtom(display, b'WM_PROTOCOLS', 0), format=32)
                    event.client.data[0] = x.XInternAtom(display, b'WM_DELETE_WINDOW', 0)
                    x.XSendEvent(display, int(window, 16), 0, 0, c.byref(event))
                    x.XFlush(display)
                    result = process.wait(timeout=10)
                    if result != 0:
                        log.seek(0)
                        raise RuntimeError('Window close failed:\n' + log.read())
                    print('PASS: 1400 navigation events without an atlas panic; clean window close')
                finally:
                    if process.poll() is None:
                        process.terminate()
                    process.wait(timeout=10)
    finally:
        x.XCloseDisplay(display)


if __name__ == '__main__':
    main()
