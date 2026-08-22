# クリーンな Linux で、配布物が本当に動くかを確かめる。
#
# **開発機では通ってしまう問題を捕まえるためにある。** 実際に次を見逃した。
#
#   - 実行時に xdotool が要ることが README に書かれていなかった
#   - 既存の設定ファイルがあると、接続先を思わぬところから拾う
#
# 配布物（AppImage）だけを入れる。ソースもツールチェーンも入れない。
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# **配布物の実行時に必要なもの。** ここに書いていないものが要るなら、
# それは README の前提条件に漏れがあるということ。
# **検証の道具はここに混ぜない**（下の層で入れる）。混ぜると、
# この一覧が実行時要件を表さなくなり、漏れを捕まえられなくなる。
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      libasound2t64 libgtk-3-0t64 libxcb1 \
      libxkbcommon-x11-0 libayatana-appindicator3-1 libegl1 libgl1 \
      xdotool xvfb procps \
      fuse3 libfuse2t64 \
    && rm -rf /var/lib/apt/lists/*

# **検証の道具。** 配布物が要求するものではない。
#   openbox / dbus-x11 / xfce4-panel … トレイのある画面を作るため
#   imagemagick / fonts-noto-cjk / tesseract … 画面を撮って文字を読むため
#   espeak-ng / sox … 検証用の合成音声を作るため（利用者の録音は使わない）
#   python3-websocket … 途中結果を受け取って数えるため
RUN apt-get update && apt-get install -y --no-install-recommends \
      openbox dbus-x11 \
      xfce4-panel xfce4-indicator-plugin \
      imagemagick fonts-noto-cjk tesseract-ocr tesseract-ocr-jpn \
      espeak-ng sox python3-websocket \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -s /bin/bash tester
WORKDIR /home/tester
USER tester

COPY --chown=tester:tester dist/*.AppImage /home/tester/otoa-input.AppImage
COPY --chown=tester:tester test-e2e/smoke.sh /home/tester/smoke.sh
COPY --chown=tester:tester test-e2e/partial_probe.py /home/tester/partial_probe.py
COPY --chown=tester:tester test-e2e/xfce4-panel.xml \
    /home/tester/.config/xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml

ENTRYPOINT ["dbus-run-session", "--", "/bin/bash", "/home/tester/smoke.sh"]
