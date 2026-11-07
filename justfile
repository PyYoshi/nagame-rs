# 利用できるコマンドを表示
default:
    @just --list

# aquaで固定した開発ツールとコミットフックを導入
setup:
    aqua install
    git config --local core.hooksPath .githooks

# このリポジトリのコミットフックを有効にする
install-hooks:
    git config --local core.hooksPath .githooks

# PDFの描画に使う固定版のPDFiumを取得（SHA256を照合してtarget/pdfiumへ展開）
pdfium:
    python3 tools/fetch_pdfium.py

# 開発用ビルド
build:
    cargo build --locked

# 通常アプリのリリースビルド（target/release/nagame）
release:
    cargo build --locked --release --bin nagame

# crossのUbuntu 24.04環境で配布用x86_64バイナリをビルド
dist-build:
    CROSS_CONTAINER_ENGINE=podman CARGO_TARGET_DIR=target/dist-linux-x86_64 \
        RUSTFLAGS="-C target-cpu=x86-64 --remap-path-prefix={{ justfile_directory() }}=." \
        aqua -c aqua.yaml exec -- cross +1.99.1 build --locked --release --bin nagame --target x86_64-unknown-linux-gnu

# 配布用ビルド環境で単体テスト（PDFの描画テストは取得済みのPDFiumを使う）
dist-test: pdfium
    CROSS_CONTAINER_ENGINE=podman CARGO_TARGET_DIR=target/dist-linux-x86_64 \
        RUSTFLAGS="-C target-cpu=x86-64 --remap-path-prefix={{ justfile_directory() }}=." \
        aqua -c aqua.yaml exec -- cross +1.99.1 test --locked --target x86_64-unknown-linux-gnu

# AppImage・ソース・ライセンス・チェックサムを生成
dist-appimage: dist-build secrets
    podman build --platform linux/amd64 -t localhost/nagame-package:ubuntu24.04 -f packaging/linux/Containerfile packaging/linux
    python3 tools/package_appimage.py

# システムlibheifのないUbuntuでAppImageのPNG/AVIF/HEIC表示を確認
dist-check-appimage: dist-appimage
    NAGAME_CODEC_FIXTURE_DIR="{{ justfile_directory() }}/target/dist/codec-fixtures" \
        CROSS_CONTAINER_ENGINE=podman CARGO_TARGET_DIR=target/dist-linux-x86_64 \
        RUSTFLAGS="-C target-cpu=x86-64 --remap-path-prefix={{ justfile_directory() }}=." \
        aqua -c aqua.yaml exec -- cross +1.99.1 test --locked --bin nagame --target x86_64-unknown-linux-gnu avif_and_heic_decode_real_encoded_images
    podman build --platform linux/amd64 -t localhost/nagame-smoke:ubuntu24.04 -f packaging/linux/Smoke.Containerfile packaging/linux
    podman run --rm --init --platform linux/amd64 -v "{{ justfile_directory() }}/target/dist/appimage:/artifacts:ro,z" -v "{{ justfile_directory() }}/target/dist/codec-fixtures:/fixtures:ro,z" -v "{{ justfile_directory() }}/tools:/checks:ro,z" localhost/nagame-smoke:ubuntu24.04 xvfb-run -a python3 /checks/check_appimage.py

# 生成済みAppImageを現在のWayland/X11と実ドライバーで確認
dist-check-appimage-host:
    python3 tools/check_appimage.py --host

# AppImageまたは通常バイナリをアプリメニューとアイコンへ登録
[positional-arguments]
install-desktop binary:
    python3 tools/install_desktop.py "$1"

# Rustコードを整形
fmt:
    cargo fmt --all

# Rustコードの整形を検査
fmt-check:
    cargo fmt --all --check

# 単体テスト
test:
    cargo test --locked

# 全ターゲット・全featureのClippy検査
clippy:
    cargo clippy --locked --all-targets --all-features -- -D warnings

# betterleaksで現在のファイルを検査（検出値はマスク）
secrets:
    aqua -c aqua.yaml exec -- betterleaks dir --config .betterleaks.toml --redact .

# ロゴのPNGを画素を変えずに最適化
optimize-png:
    aqua -c aqua.yaml exec -- oxipng -o 4 --strip safe assets/branding/*.png

# 基本チェックを順に実行（失敗した時点で停止）
check: fmt-check test clippy secrets build
