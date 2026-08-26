#!/usr/bin/env bash
# macOS 配布物を作る。Mac 実機で実行する。
#
# .app を作り、Developer ID で署名し、DMG にして Apple の公証を通す。
# **署名なしの macOS バイナリは Gatekeeper に弾かれる。** ダウンロードした人が
# ダブルクリックで開けるようにするには、ここまで必要である。
#
# 必要なもの（環境変数で場所を渡す）:
#   OTOA_SIGN_CERT_PEM   Developer ID の証明書 (PEM)
#   OTOA_SIGN_KEY_PEM    その秘密鍵 (PEM)
#   OTOA_ASC_KEY         App Store Connect API キー (.p8)
#   OTOA_ASC_KEY_ID      その key-id
#   OTOA_ASC_ISSUER      その issuer-id
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[ "$(uname -s)" = Darwin ] || { echo "Mac 実機で実行する" >&2; exit 1; }

for v in OTOA_SIGN_CERT_PEM OTOA_SIGN_KEY_PEM OTOA_ASC_KEY OTOA_ASC_KEY_ID OTOA_ASC_ISSUER; do
    [ -n "${!v:-}" ] || { echo "$v が未設定" >&2; exit 1; }
done

WORK=/tmp/otoa-macos-package
# DMG の元にするフォルダ。**ここに入れたものが利用者へ渡る。**
# 鍵や中間ファイルを置いてはいけないし、DMG 自体もここへ書いてはいけない
# （自分を取り込みながら膨らみ、空き容量を食い潰す）。
STAGE="$WORK/dmgroot"
APP="$STAGE/Otoa Input.app"
SECRETS="$WORK/secrets"
KC=/tmp/otoa-sign.keychain-db
# 使い捨てキーチェーンの合鍵。**固定値を書かない。**
# このキーチェーンは署名のたびに作って捨てるので、毎回作れば十分である。
KC_PASS="$(head -c 32 /dev/urandom | base64 | tr -d '/+=' | head -c 24)"
DMG="$WORK/OtoaInput-$VERSION.dmg"

echo "==> ビルド"
cargo build --release

echo "==> .app を作り直す"
# **毎回まっさらに作る。** 失敗した署名の残骸(*.cstemp / _CodeSignature)が
# 残っていると invalid or unsupported format for signature になる。
rm -rf "$WORK"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$SECRETS"
cp target/release/otoa-input target/release/otoa-asr-server "$APP/Contents/MacOS/"
cp resources/icons/otoa-input-192.png "$APP/Contents/Resources/icon.png"

# 第三者ライセンスの表示を .app の中に入れる。
bash scripts/generate-licenses.sh
cp LICENSE NOTICE THIRD-PARTY-LICENSES.md "$APP/Contents/Resources/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Otoa Input</string>
  <key>CFBundleDisplayName</key><string>Otoa Input</string>
  <key>CFBundleIdentifier</key><string>jp.otoa.input</string>
  <key>CFBundleExecutable</key><string>otoa-input</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSMicrophoneUsageDescription</key><string>話した内容を文字にするためにマイクを使います。</string>
</dict></plist>
PLIST

cat > "$SECRETS/otoa.entitlements" <<'ENT'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.device.audio-input</key><true/>
  <key>com.apple.security.cs.allow-jit</key><true/>
  <key>com.apple.security.cs.disable-library-validation</key><true/>
</dict></plist>
ENT

echo "==> 署名用キーチェーン"
# **終わったら必ず消す。** 残すと、Developer ID の秘密鍵を含むキーチェーンが
# /tmp に置きっぱなしになる。
cleanup_keychain() {
    security default-keychain -s ~/Library/Keychains/login.keychain-db 2>/dev/null || true
    security list-keychains -d user -s ~/Library/Keychains/login.keychain-db 2>/dev/null || true
    security delete-keychain "$KC" 2>/dev/null || true
    rm -f "$SECRETS/devid.p12" 2>/dev/null || true
}
trap cleanup_keychain EXIT
# SSH 越しの非対話セッションでは秘密鍵の使用許可ダイアログを出せず、
# codesign が errSecInternalComponent で失敗するかハングする。
# 専用キーチェーンを作り、partition-list を設定して既定にするのが回避策。
security delete-keychain "$KC" 2>/dev/null || true
security create-keychain -p "$KC_PASS" "$KC"
security set-keychain-settings "$KC"          # 自動ロックを無効化
security unlock-keychain -p "$KC_PASS" "$KC"
openssl pkcs12 -export -inkey "$OTOA_SIGN_KEY_PEM" -in "$OTOA_SIGN_CERT_PEM" \
    -out "$SECRETS/devid.p12" -passout "pass:$KC_PASS" -name "Developer ID Application"
security import "$SECRETS/devid.p12" -k "$KC" -P "$KC_PASS" -A -f pkcs12
# **ここの終了コードを握り潰さない。** 失敗していると署名が必ずハングする。
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KC_PASS" "$KC" > /dev/null
security list-keychains -d user -s "$KC" ~/Library/Keychains/login.keychain-db
security default-keychain -s "$KC"
rm -f "$SECRETS/devid.p12"   # キーチェーンへ入れたら不要

IDENTITY="$(security find-identity -v -p codesigning "$KC" | grep -o '[0-9A-F]\{40\}' | head -1)"
[ -n "$IDENTITY" ] || { echo "署名 ID が見つからない" >&2; exit 1; }
echo "    識別子: $IDENTITY"

echo "==> 署名"
# 内側から署名する。入れ子のバイナリを先に署名しないと、外側の署名が壊れる。
security unlock-keychain -p "$KC_PASS" "$KC"
codesign --force --options runtime --timestamp \
    --entitlements "$SECRETS/otoa.entitlements" --sign "$IDENTITY" \
    "$APP/Contents/MacOS/otoa-asr-server"
codesign --force --options runtime --timestamp \
    --entitlements "$SECRETS/otoa.entitlements" --sign "$IDENTITY" "$APP"

codesign -dv --verbose=2 "$APP" 2>&1 | grep -E "Authority=|TeamIdentifier=|Timestamp=" || true
codesign -dv --verbose=2 "$APP" 2>&1 | grep -q "TeamIdentifier=not set" \
    && { echo "アドホック署名になっている" >&2; exit 1; }

echo "==> DMG"
# ドラッグでインストールできるように Applications への symlink を置く。
ln -sfn /Applications "$STAGE/Applications"
# 念のため、渡してはいけないものが混ざっていないか確かめる。
if find "$STAGE" -name "*.p12" -o -name "*.pem" -o -name "*.p8" | grep -q .; then
    echo "DMG に入れるフォルダへ鍵が混ざっている" >&2; exit 1
fi
hdiutil create -volname "Otoa Input" -srcfolder "$STAGE" -ov -format UDZO "$DMG" > /dev/null
codesign --force --sign "$IDENTITY" "$DMG"

echo "==> 公証（2〜5 分）"
xcrun notarytool submit "$DMG" \
    --key "$OTOA_ASC_KEY" --key-id "$OTOA_ASC_KEY_ID" --issuer "$OTOA_ASC_ISSUER" \
    --wait --timeout 20m

echo "==> staple"
xcrun stapler staple "$DMG"
spctl -a -vvv -t install "$DMG" 2>&1 | tee "$SECRETS/spctl.txt"
grep -q "source=Notarized Developer ID" "$SECRETS/spctl.txt" \
    || { echo "公証が反映されていない" >&2; exit 1; }

mkdir -p "$ROOT/dist"
# 名前にバージョンを入れない（build-release.sh と同じ理由）。
# .app の CFBundleShortVersionString には入っているので、版は失われない。
cp "$DMG" "$ROOT/dist/otoa-input-macos-arm64.dmg"
( cd "$ROOT/dist" && shasum -a 256 "otoa-input-macos-arm64.dmg" \
    > "otoa-input-macos-arm64.dmg.sha256" )
echo
echo "できた: $ROOT/dist/otoa-input-macos-arm64.dmg"
