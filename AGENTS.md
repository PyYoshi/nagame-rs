# Nagame 開発ガイド

## セッション開始時

1. 操作・機能・制限は [README.md](README.md)、設計は [docs/architecture.md](docs/architecture.md)、開発・検証は [docs/development.md](docs/development.md) を参照する。
2. 作業ディレクトリは、この `AGENTS.md` と `Cargo.toml` があるプロジェクトルート。表示名は **Nagame（ながめ）**、Cargoパッケージ・通常バイナリ・app_idは `nagame`。
3. 現在の実装と利用者の新しい依頼を起点に作業する。未依頼の機能を必須要件として増やさない。

## コードの発見

- 場所が分からない挙動を調べるときは、最初にjbcontextの `code_search` を使う。自然言語で、一度に一つの目的を検索する。
- 関連ファイルを見つけたら、まずそのファイルを読む。以後はその範囲を直接読むか `rg` で正確に検索する。
- ファイル・関数・シンボルが既知の場合、ビルド・設定変更・既知の差分レビューでは、意味検索を挟まず直接操作する。
- 同じ領域を再び意味検索する場合は `pathFilter` で絞る。

## 実装上の注意

- 利用者向け文言は `locales/{ja,en}/nagame.ftl` に置き、`src/i18n.rs` の `tr!` で参照する。初回はOSの主言語が日本語なら日本語、それ以外は英語。保存した手動指定を優先する。選定理由と検証上の注意は [多言語対応](docs/localization.md) を参照する。

- GPUIへ渡す画像は **BGRA**。保存・外部コピーのRGBA変換と混同しない。
- `Rgba32FImage` の補間入力は **0〜1**。アルファも正規化し、補間後に色・アルファとも8bitへ戻す。
- 表示用リサイズは非同期。元画像を維持し、保存・コピーに表示解像度の画像を使わない。
- 古いデコード・補間結果を現在の画像へ反映させない。世代管理を維持する。
- テクスチャを `render` 中に即時解放しない。現在の2フレームの遅延解放と、再利用中画像の除外を維持する。
- 画素データの上限は固定値を増やさず `memory::Budget` から取る。保持する画像が変わったら台帳（`memory::Ledger`）を更新する。理由と導出は [メモリー予算](docs/architecture.md#メモリー予算) を参照する。
- GIFはチャンク（数フレームの `RenderImage`）単位で保持・退役し、`Player` が選んだフレームを `canvas` で描く。複数フレームの画像をGPUIの `img` に渡さない（自前でコマ送りし続けるため）。
- PDFiumは補助プロセス（`src/pdf/helper.rs`）だけが呼ぶ。本体へPDFiumのハンドルや `pdfium-render` の型を持ち込まず、補助プロセスの応答は寸法・大きさを検査してから使う。PDFのページは基準解像度で描いた画像を通常の画像として扱い、表示解像度の描き直しを保存・コピーに使わない。理由と未決事項は [PDF対応](docs/pdf-support.md) を参照する。
- `vendor/xattr` はGPUIの間接依存に対する互換性パッチ。現在の依存構成ではパッチと同梱ライセンスを保持する。理由・他プロジェクトの実例・削除条件は [開発・検証](docs/development.md#xattrの互換性パッチ) を参照する。
- システム依存ライブラリが必要になったら、理由と導入コマンドを利用者へ提案する。利用者がインストールする方針。

## 検証

変更に合ったチェックを行う。文書のみの変更でGUIテストを再実行する必要はない。

非公開の検証資料は原本を読み取り、リポジトリー・`target`・配布物へコピーしない。ファイル名、著者・タイトル等のメタデータ、本文、ページ画像を文書・ログ・スクリーンショット・コミットへ残さない。回帰テストには生成した合成データを使う。

開発コマンドは `just` で一覧を表示できる。PDFのテストと表示には `just pdfium` で取得する固定版のPDFiumが必要（`cargo test` の前に実行する）。`aqua install` の後、`just release` でリリースビルド、`just secrets` で秘密情報の混入確認、`just check` で以下の基本チェックと秘密情報検査をまとめて実行できる。固定版CLIと除外範囲は [開発・検証](docs/development.md#開発ツールと秘密情報の検査) を参照する。

```sh
cargo fmt --all --check
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --locked
```

描画・入力・設定の回帰確認には、表示環境がある場合に `cargo build --locked --features ui-smoke` と `python3 tools/check_ui.py` を使う。最後に `cargo build --locked` で通常バイナリへ戻す。テストの範囲と手動確認項目は [開発・検証](docs/development.md) を参照する。

設計・コマンド・重要な制限を変更したら、`README.md`・該当する開発文書を現在の状態に更新する。
