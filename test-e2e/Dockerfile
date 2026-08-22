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

# **実行時に必要なものだけ**を入れる。ここに書いていないものが要るなら、
# それは README の前提条件に漏れがあるということ。
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      libasound2t64 libgtk-3-0t64 libxcb1 \
      xdotool xvfb \
      fuse3 libfuse2t64 \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -s /bin/bash tester
WORKDIR /home/tester
USER tester

COPY --chown=tester:tester dist/*.AppImage /home/tester/otoa-input.AppImage
COPY --chown=tester:tester test-e2e/smoke.sh /home/tester/smoke.sh

ENTRYPOINT ["/bin/bash", "/home/tester/smoke.sh"]
