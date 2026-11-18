# SPDX-License-Identifier: GPL-3.0-or-later
FROM docker.io/library/ubuntu:24.04@sha256:77eb101c607d6e3d4f5ad0c8a02c2136c689b98aba1f359e6502ba517e5c57ec
ENV DEBIAN_FRONTEND=noninteractive
# システムのlibheifとプラグインはあえて入れない。デコードにはAppImageの同梱分を使わせる。
RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 libstdc++6 libvulkan1 mesa-vulkan-drivers xvfb xauth \
        fontconfig fonts-dejavu-core dbus-x11 \
    && rm -rf /var/lib/apt/lists/*
