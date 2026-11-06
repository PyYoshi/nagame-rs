# 利用できるコマンドを表示
default:
    @just --list

# aquaで固定した開発ツールをインストール
setup:
    aqua install

# 開発用ビルド
build:
    cargo build --locked

# 通常アプリのリリースビルド（target/release/nagame）
release:
    cargo build --locked --release --bin nagame

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

# 基本チェックを順に実行（失敗した時点で停止）
check: fmt-check test clippy secrets build
