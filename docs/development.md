# 開発・検証

## 開発環境

以下のコマンドは `Cargo.toml` があるプロジェクトルートで実行する。ビルドに必要なシステムライブラリと導入例は [README](../README.md#ビルド環境) を参照する。追加のシステムパッケージが必要な場合は、理由と導入コマンドを利用者へ提案し、導入は利用者が行う。

`Cargo.toml` の最小Rustバージョンは1.88、検証済み環境はRust 1.99.1、Nobara/Fedora系Linux。GPUIは0.2.2に固定し、依存は `Cargo.lock` に従う。開発ビルドも画像処理の速度を保つため最適化を有効にしている。表示にはWaylandまたはX11とVulkanが必要。

```sh
cargo build --locked
./target/debug/nagame /path/to/image.jpg
# リリースビルドで起動する場合
cargo run --locked --release -- /path/to/images
```

## 開発ツールと秘密情報の検査

開発用CLIは [aqua](https://aquaproj.github.io/) で管理する。aquaが未導入の場合は [公式の導入手順](https://aquaproj.github.io/docs/install) に従い、`aqua` をPATHから実行できるようにする。この環境ではaqua 2.65.2で確認した。

[aqua.yaml](../aqua.yaml) でレジストリと開発・配布ツールのバージョンを固定し、[aqua-checksums.json](../aqua-checksums.json) に配布物のチェックサムを記録している。現在のjustは1.59.0、betterleaksはv1.10.0、oxipngはv10.2.1、crossはv0.2.5。AppImage用ツールの版は [配布用ビルド](distribution.md) を参照。インストール時はチェックサム照合を必須とする。

AppImage用の4ツールは標準レジストリにないため、[ローカルレジストリ](../packaging/linux/aqua-registry.yaml) を使う。初回は [aqua-policy.yaml](../aqua-policy.yaml) の許可対象を確認し、信頼登録してからインストールする。ポリシーを変更した場合は再確認・再登録する。全ポリシーの無効化は行わない。

```sh
aqua policy allow aqua-policy.yaml
aqua install
just secrets
```

開発コマンドは [justfile](../justfile) にまとめている。`just` だけで一覧を表示する。aqua管理のbinディレクトリがPATHにない場合や、別のjustが優先される環境では `aqua exec -- just <コマンド>` で固定版を実行できる。

| コマンド | 内容 |
|---|---|
| `just setup` | aquaの開発ツールをインストール |
| `just build` | 開発用ビルド |
| `just release` | 通常アプリのリリースビルド。出力は `target/release/nagame` |
| `just dist-build` / `just dist-test` | crossとPodmanによる配布用ビルド／単体テスト。[環境と出力先](distribution.md) |
| `just dist-appimage` | AppImage、ソースアーカイブ、ライセンス一覧、ビルド記録、SHA256を生成 |
| `just dist-check-appimage` | AppImage生成後、システムlibheifのないUbuntuでPNG・8/10bit AVIF/HEICのGUI読み込みを検証 |
| `just dist-check-appimage-host` | 生成済みAppImageを現在のデスクトップのWayland/X11とGPUドライバーで検証。画像読み込み後にテスト用ウィンドウを閉じる |
| `just install-desktop BINARY` | 指定したAppImage／通常バイナリの起動項目とNagameアイコンをユーザー領域に登録 |
| `just fmt` / `just fmt-check` | Rustコードの整形／整形検査 |
| `just test` | 単体テスト |
| `just clippy` | 全ターゲット・全featureのClippy検査 |
| `just secrets` | betterleaksで現在のファイルを検査 |
| `just optimize-png` | `assets/branding/*.png` をoxipngで最適化。`-o 4 --strip safe` を使い、透明画素のRGBを変更する `--alpha` は使わない |
| `just check` | 整形検査→テスト→Clippy→秘密情報検査→開発用ビルド。失敗時は停止 |

`just secrets` はjustfileのあるプロジェクトルートで、aqua経由の固定版betterleaksを直接実行する。Git未追跡のファイルも含め、現在のファイルを検査する。履歴の検査ではない。検出がなければ終了コード0、検出時は1を返す。

[.betterleaks.toml](../.betterleaks.toml) を明示的に読み込み、組み込みの検出ルールを継承する。`target`、`.git`、`__pycache__` ディレクトリとIDEの `workspace.xml` を除外する。検出値は `--redact` でマスクし、外部APIによる有効性検証は有効にしない。

Git履歴を検査する場合は、コミットが存在するリポジトリで次を使う。

```sh
aqua exec -- betterleaks git --config .betterleaks.toml --redact .
```

開発ツールの版を更新する場合は `aqua.yaml` を編集した後、チェックサムを更新して再検査する。betterleaksのチェックサム取得時は標準レジストリに定義されたCosign署名検証も行われる。

```sh
aqua update-checksum --prune
aqua install
just secrets
```

## xattrの互換性パッチ

GPUI 0.2.2を現在の依存構成でLinux向けにビルドするため、`xattr 0.2.3` の修正版を `vendor/xattr` に同梱している。Nagameから直接使う依存ではなく、次の経路で取り込まれる。

```text
nagame → gpui → gpui_http_client → zed-async-tar → xattr 0.2.3
```

元の `xattr 0.2.3` は `libc::ENOATTR` を参照するが、libc 0.2.190以降ではLinux/Android向けのこの別名が削除され、コンパイルできない。Linux/Androidでは「拡張属性が存在しない」ことを表す `libc::ENODATA` を使う。修正箇所は [vendor/xattr/src/sys/mod.rs](../vendor/xattr/src/sys/mod.rs) で、macOS/BSDの `ENOATTR` は維持している。

[Cargo.toml](../Cargo.toml) の `[patch.crates-io]` で、間接依存を含めてこのローカル修正版へ差し替える。これは依存先の不具合を修正するためにCargoが公式に用意している仕組み。[Cargo公式文書](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html) に、上流の修正公開を待つ間の利用例がある。

同じ対応の公開例として、`ntfy-gpui` もGPUI 0.2.2向けに修正版を `vendor/xattr` へ同梱している。[同プロジェクトのパッチ説明](https://tangled.org/madoka.systems/ntfy-gpui/blob/main/vendor/xattr/PATCH.md) を参照。ただし全てのLinux向けGPUIプロジェクトに必要な処置ではなく、GPUIと間接依存のバージョンの組み合わせによる。

別の回避策として、`gpui-updater` はlibcを0.2.189へ固定する方法を案内している。[同プロジェクトの開発資料](https://docs.rs/crate/gpui-updater/0.4.0) を参照。Nagameではlibcを古い版に固定せず、問題の定数参照だけを修正する方法を採用している。

### 維持と削除の条件

現状ではパッチ指定・修正版・同梱のMIT/Apacheライセンスを一緒に保持する。Cargoのレジストリキャッシュを直接編集すると他の環境で再現できないため、変更は同梱ソースで管理する。

GPUI等の依存更新時に、次のコマンドで経路を再確認する。

```sh
cargo tree --locked -i xattr
```

依存元の更新で未修正の `xattr 0.2.3` が不要になったら、パッチ指定と同梱ソースの削除を検討する。削除時はロックファイルも更新し、基本チェックを通してから、この説明とAGENTS.mdの注意事項を更新する。現在の依存構成のままパッチだけを削除しない。

## 変更箇所を探す

構成と処理順序は [コード構成と設計](architecture.md) を参照する。以下は機能変更時に確認する範囲の目安。

| 変更内容 | 主な確認先 |
|---|---|
| 表示文言・翻訳 | `locales/{ja,en}/nagame.ftl`、`src/i18n.rs`。[選定理由と追加手順](localization.md) |
| 操作の追加、キー割り当て | `main.rs` の `Command` / `command` / `shortcut_command`、`bindings.rs` の `Shortcut` / `defaults`、`menu_ui.rs` |
| 設定の追加 | `settings.rs` のフィールド・既定値・`normalize`、`settings_ui.rs` の `Field` / タブ / 表示・編集処理、`main.rs` の適用処理 |
| 入力形式の追加 | `model.rs::supported`、`decoder.rs` のデコードと `dimensions`、実コーデックのテスト |
| 出力形式の追加 | `viewer_io.rs` の形式許可・エンコード・RGBA変換、保存失敗時の既存ファイル保持テスト |
| 画像送り・フォルダ移動 | `model.rs::Album`、`main.rs` の境界処理・見開きのページ送り、`pipeline.rs` の合成 |
| 補間・GPU画像の寿命 | `resample.rs`、`main.rs` の `clear_image` / `render`、`ui_smoke.rs` |

設定フィールド追加時は古いJSONからの既定値補完を保ち、保存→読み込みでも値が維持されることを確認する。`Settings::normalize` はスライド秒数を1〜3,600、スキップ枚数を1〜10,000、背景色を24bitに制限する。設定ファイルの手書き編集より、通常は設定画面を使う。

GPUIの `use gpui::*` はテスト用マクロも導入する。通常の `#[test]` を書くモジュールでは、必要な型を明示的にimportしてマクロ衝突を避ける。

## 基本チェック

実装変更時は変更に合う範囲を実行する。文書だけの変更ではリンク・記述の整合性を確認し、GUIテストは再実行しなくてよい。

```sh
cargo fmt --all --check
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --locked
```

テストは各モジュール内の `#[cfg(test)] mod tests` にある。実装上の理由と確認対象を対応させ、既存の回帰テストを維持する。

| 対象 | 主な検証内容 |
|---|---|
| `i18n` | Fluentの構文と重複、両言語のキー集合、全メッセージの整形、OS言語の対応付け、単数・複数形、英語へのフォールバック、並行呼び出し、利用者の文字列の保持 |
| `model` | 数値順、空一覧、選択の維持、フォルダ探索、倍率とパン |
| `decoder` | BGRAと回転、JPE、破損ファイル、GIFの時間・先行表示・中止、実AVIF/HEIC |
| `pipeline` | キャッシュ再利用とファイル更新、見開きの左右・横長・回転、幅0のページの拒否とワーカーの継続 |
| `resample` | 古い結果の破棄、上限時のGPU復帰、最近傍、透明な縁、不透明・半透明の色とアルファ |
| `settings` | 保存・再読み込み、古いファイルの既定値補完、破損JSON |
| `scrollbar` | バー位置とスクロール量、両端への移動 |
| `viewer_io` | 出力色、JPE、コーデック失敗時の既存ファイル保持 |

AVIF/HEICのテストは名前だけの疑似ファイルではなく、AV1/HEVCを実際にエンコード・デコードする。libheif本体と対応するデコーダーに加え、テストではエンコーダーも必要になる。

## 実ウィンドウの回帰テスト

描画・入力・設定の変更時は表示環境があれば実行する。検証済みの経路はX11。`tools/check_ui.py` は子プロセスの `WAYLAND_DISPLAY` を外すため、X11へ接続できる環境が必要。

```sh
cargo build --locked --features ui-smoke
python3 tools/check_ui.py
# 検証後は通常バイナリへ戻す（失敗した場合も同様）
cargo build --locked
```

スクリプトは一時ディレクトリに生成画像と設定を置き、1,400回の画像送り、設定のキャンセル・保存・再起動、言語切替、セレクター、キー変更、見開き・横長ページ・読む方向、補間後のアルファと色、正常終了を検証する。実ウィンドウを描画するが、キーイベントは `ui_smoke.rs` がアプリ内へ配信する。OSからのマウス・DnD・外部アプリへの貼り付けは下記の手動回帰確認で扱う。

`ui-smoke` は専用featureであり、通常バイナリでは `NAGAME_UI_SMOKE` を設定しても起動しない。テストの一時設定を使うため、通常の利用者設定は変更しない。

`python3 tools/stress_navigation.py` はX11の外部入力を使う補助検証。libX11 / libXtst / xpropが必要で、専用ウィンドウだけに計1,400回の入力を送り、生存と移動方向を確認する。過去に成功した環境もあるが、直近の環境では合成入力が届かなかった。通常の回帰確認には `check_ui.py` を用い、外部入力の配送成功とは区別して扱う。

## 手動回帰確認

変更に応じて以下を実機で確認する。自動テストのアプリ内入力だけでは、OS経由の入力や外部アプリとの相互運用を保証しない。

| 対象 | 確認内容 |
|---|---|
| ファイル入力 | 選択ダイアログのキャンセル、画像／フォルダのDnD、日本語・スペースを含むパス、破損画像のエラー後の復帰 |
| 閲覧と描画 | 数値順と画像端の動作、フォルダ移動、ズーム・パンの制限、EXIF、回転4回、GIF切替、全画面、自動送り、終了 |
| 非同期処理 | 読み込み・補間中の連続画像送りとズーム後に、最終選択の画像・倍率を表示すること |
| 設定UI | 保存と再起動、キャンセル、キーの変更・解除、スクロールバーの実クリック／ドラッグ、固定位置の保存ボタン |
| セレクター | 候補のクリック決定、Esc／外側クリックで未確定候補を破棄、画面下部でも一覧が切れないこと |
| マンガ | 縦2枚→横1枚→縦2枚の前進・後退、RTL/LTR、横長単独表示、合成上限時の1ページ表示 |
| 表示補間 | 小さなピクセル画像の拡大と写真の縮小、最近傍とフィルターの差、不透明・半透明画像の色とアルファ |
| 外部出力 | 回転・見開き後の保存と再読込、GIFの時間、JPEの入出力、外部アプリへの画像／パス貼り付け、フォルダを開く操作 |

直近の実機確認では、X11の画像表示・画像送り・フォルダ移動・画像端の選択を確認済み。全項目を手動検証済みではなく、実マウスによるスクロールバー・セレクター、OSからのDnD、外部アプリへの画像貼り付けは未確認として残る。

## 性能調査

```sh
# GUIなし：ヘッダー取得、デコード・向き・BGRA変換の時間
cargo run --locked --bin decode-bench -- /path/to/image.jpg /path/to/image.png
# GUIあり：読み込み時間、キャッシュヒット、GIFの部分／全体応答
NAGAME_PROFILE=1 ./target/debug/nagame /path/to/image.jpg
```

デコード計測にはGPUアップロードや描画時間を含まない。GUIログの `cached=true` はデコード済みキャッシュのヒットであり、`0 ms` はミリ秒単位の丸め。入力から画面表示まで0msという意味ではない。比較するときは同じ画像・ビルドプロファイル・コーデック環境で複数回測り、キャッシュの有無を分ける。

## 文書の更新

操作・制限が変わったらREADMEを更新する。処理の責務や不変条件が変わったら `architecture.md`、開発コマンド・検証方法が変わったらこの文書を更新する。

ソースのモジュールコメントと内部型の説明は以下でHTMLとして確認できる。

```sh
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps --document-private-items
```

通常バイナリの入口は `target/doc/nagame/index.html`。
