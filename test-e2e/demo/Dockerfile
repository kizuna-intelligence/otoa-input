# デモ動画を撮る環境。
# Xvfb 上でアプリを動かし、仮想マイクへ音声を流し込んで画面を録る。
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates libasound2t64 libgtk-3-0t64 libxcb1 \
      xdotool xvfb x11-utils openbox \
      libxkbcommon-x11-0 libayatana-appindicator3-1 \
      libegl1 libgl1 libgl1-mesa-dri libglib2.0-0t64 \
      libxrandr2 libxi6 libxcursor1 libxcomposite1 libxdamage1 libxfixes3 \
      libxrender1 libxext6 libfontconfig1 libfreetype6 \
      pulseaudio pulseaudio-utils alsa-utils \
      ffmpeg mousepad \
      fonts-noto-cjk \
      fuse3 libfuse2t64 procps \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -m -s /bin/bash demo
USER demo
WORKDIR /home/demo
