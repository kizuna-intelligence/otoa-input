#!/usr/bin/env python3
"""合成 PCM を Otoa ASR Protocol v1 で送り、kodama の途中結果を確認する。"""

import json
import pathlib
import sys
import time

import websocket


def receive_available(ws, partials):
    while True:
        try:
            message = ws.recv()
        except websocket.WebSocketTimeoutException:
            return
        if not isinstance(message, str):
            continue
        if not message:
            return
        response = json.loads(message)
        if response.get("error_code") is not None:
            raise RuntimeError(f"server error: {response}")
        for token in response.get("tokens", []):
            text = token.get("text", "")
            if text and not token.get("is_final", False):
                partials.append(text)


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: partial_probe.py <16kHz-mono-s16le.pcm>")

    pcm = pathlib.Path(sys.argv[1]).read_bytes()
    if not pcm:
        raise SystemExit("PCM is empty")

    ws = websocket.create_connection("ws://127.0.0.1:8770/asr/v1", timeout=5)
    ws.settimeout(0.02)
    ws.send(
        json.dumps(
            {
                "model": "stt-rt-v5",
                "audio_format": "pcm_s16le",
                "sample_rate": 16000,
                "num_channels": 1,
                "language_hints": ["ja"],
                "enable_endpoint_detection": True,
                "endpoint_mode": "client",
            }
        )
    )

    partials = []
    # 64 ms ごとに送る。デコード用タスクの完了後にも次のフレームが届くため、
    # サーバーが非 final の結果を WebSocket へ流せる。
    chunk_bytes = 1024 * 2
    for offset in range(0, len(pcm), chunk_bytes):
        ws.send_binary(pcm[offset : offset + chunk_bytes])
        receive_available(ws, partials)
        if partials:
            break
        time.sleep(0.064)

    # 最後の音声片でデコードが始まった場合にも、結果を取り出す契機を作る。
    for _ in range(30):
        if partials:
            break
        ws.send_binary(bytes(chunk_bytes))
        time.sleep(0.064)
        receive_available(ws, partials)

    ws.close()
    if not partials:
        raise SystemExit("non-final token count: 0")
    print(f"non-final token count: {len(partials)}")
    print(f"first partial: {partials[0]}")


if __name__ == "__main__":
    main()
