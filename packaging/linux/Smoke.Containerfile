# SPDX-License-Identifier: GPL-3.0-or-later
FROM docker.io/library/ubuntu:24.04@sha256:78d4744ead1f5b19b04d0df558ab182a5300903900755e515b2ca4d10eb2044d
ENV DEBIAN_FRONTEND=noninteractive
# Deliberately no system libheif/plugins: decoding must use the AppImage payload.
RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 libstdc++6 libvulkan1 mesa-vulkan-drivers xvfb xauth \
        fontconfig fonts-dejavu-core dbus-x11 \
    && rm -rf /var/lib/apt/lists/*
