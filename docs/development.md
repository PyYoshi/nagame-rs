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

PDFの表示とテストには、固定版のPDFiumが必要。`just pdfium`（`python3 tools/fetch_pdfium.py`）が [pdfium.json](../packaging/linux/pdfium.json) の版を取得し、SHA256を照合して `target/pdfium/` へ展開する。`gh` があれば出所の証明（attestation）も検証する。版を更新するときは `--require-attestation` で証明の検証を必須にする。`cargo run` と `cargo test` には [.cargo/config.toml](../.cargo/config.toml) が `NAGAME_PDFIUM_LIBRARY` を設定する。`./target/debug/nagame` を直接起動してPDFを開く場合は、同じ環境変数に `target/pdfium/chromium-8125/lib/libpdfium.so` の絶対パスを渡す。PDFiumは補助プロセス（`nagame --pdf-helper`）だけがロードする。設計は [PDF対応](pdf-support.md) を参照。

## 開発ツールと秘密情報の検査

開発用CLIは [aqua](https://aquaproj.github.io/) で管理する。aquaが未導入の場合は [公式の導入手順](https://aquaproj.github.io/docs/install) に従い、`aqua` をPATHから実行できるようにする。この環境ではaqua 2.65.2で確認した。

[aqua.yaml](../aqua.yaml) でレジストリと開発・配布ツールのバージョンを固定し、[aqua-checksums.json](../aqua-checksums.json) に配布物のチェックサムを記録している。現在のjustは1.59.0、betterleaksはv1.10.0、oxipngはv10.2.1、crossはv0.2.5。AppImage用ツールの版は [配布用ビルド](distribution.md) を参照。インストール時はチェックサム照合を必須とする。

AppImage用の4ツールは標準レジストリにないため、[ローカルレジストリ](../packaging/linux/aqua-registry.yaml) を使う。初回は [aqua-policy.yaml](../aqua-policy.yaml) の許可対象を確認し、信頼登録してからインストールする。ポリシーを変更した場合は再確認・再登録する。全ポリシーの無効化は行わない。

```sh
aqua policy allow aqua-policy.yaml
aqua exec -- just setup
just secrets
```

`just setup` はツールの導入後、このリポジトリの `core.hooksPath` を `.githooks` に設定する。ツールが導入済みの場合は `just install-hooks` だけでも有効にできる。グローバルなGit設定は変更しない。別のフックを既に使っている環境では、設定を切り替える前に内容を確認する。

開発コマンドは [justfile](../justfile) にまとめている。`just` だけで一覧を表示する。aqua管理のbinディレクトリがPATHにない場合や、別のjustが優先される環境では `aqua exec -- just <コマンド>` で固定版を実行できる。

| コマンド | 内容 |
|---|---|
| `just setup` | aquaの開発ツールをインストールし、コミットフックを有効化 |
| `just install-hooks` | このリポジトリのコミットフックだけを有効化 |
| `just build` | 開発用ビルド |
| `just release` | 通常アプリのリリースビルド。出力は `target/release/nagame` |
| `just dist-build` / `just dist-test` | crossとPodman（PodmanがなくDockerがあればDocker）による配布用ビルド／単体テスト。[環境と出力先](distribution.md) |
| `just dist-appimage` | AppImage、ソースアーカイブ、ライセンス一覧、ビルド記録、SHA256を生成 |
| `just dist-check-appimage` | AppImage生成後、システムlibheifのないUbuntuでPNG・8/10bit AVIF/HEIC・拡張子と中身が違うファイルのGUI読み込みを検証 |
| `just dist-check-appimage-host` | 生成済みAppImageを現在のデスクトップのWayland/X11とGPUドライバーで検証。画像読み込み後にテスト用ウィンドウを閉じる |
| `just install-desktop BINARY` | 指定したAppImage／通常バイナリの起動項目とNagameアイコンをユーザー領域に登録 |
| `just fmt` / `just fmt-check` | Rustコードの整形／整形検査 |
| `just test` | 単体テスト |
| `just clippy` | 全ターゲット・全featureのClippy検査 |
| `just secrets` | betterleaksで現在のファイルを検査 |
| `just optimize-png` | `assets/branding/*.png` をoxipngで最適化。`-o 4 --strip safe` を使い、透明画素のRGBを変更する `--alpha` は使わない |
| `just check` | 整形検査→テスト→Clippy→秘密情報検査→開発用ビルド。失敗時は停止 |

`just secrets` はjustfileのあるプロジェクトルートで、aqua経由の固定版betterleaksを直接実行する。Git未追跡のファイルも含め、現在のファイルを検査する。履歴の検査ではない。検出がなければ終了コード0、検出時は1を返す。

[.betterleaks.toml](../.betterleaks.toml) を明示的に読み込み、組み込みの検出ルールを継承する。`target`、`.git`、`.omc`、`__pycache__` ディレクトリとIDEの `workspace.xml` を除外する。`.omc` はoh-my-claudecodeのローカル動作データで、Gitと配布用ソースからも除外されている。セッション制御用の値をアプリの秘密情報として検出しないよう、パスを限定して除外する。Gitのignore対象を一律に検査から外すことはせず、`.env` などを含む他のファイルには引き続き組み込みルールを適用する。検出値は `--redact` でマスクし、外部APIによる有効性検証は有効にしない。

Git履歴を検査する場合は、コミットが存在するリポジトリで次を使う。

```sh
aqua exec -- betterleaks git --config .betterleaks.toml --redact .
```

有効化した [.githooks/pre-commit](../.githooks/pre-commit) は、コミット直前に固定版betterleaksを `git --pre-commit --staged` で実行する。ステージされた差分に検出があるとコミットを停止し、値をマスクしてファイル・行などを表示する。作業ツリーだけにある未ステージの内容は、このフックの検査対象ではない。ステージ後に作業ツリーから値を消しただけでは通過せず、修正内容の再ステージが必要になる。フック・設定・必要なツールを実行できない場合もコミットを停止する。

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
| 入力形式の追加 | `model.rs::supported`、`decoder.rs` の中身による判定（`sniff`）・デコード・`dimensions`・`format_name`、実コーデックのテスト |
| PDF | `pdf/mod.rs`（公開API・仲介スレッド・補助プロセスの起動と再起動・応答の検査）、`pdf/helper.rs`（PDFium・分類と基準解像度・memfdへの描画）、`pdf/ipc.rs`、`model.rs` の `Entry` / `Album::open_document` / `directories`、`pipeline.rs` の `render_page` / `common_scale`、`resample.rs` の `redraw` |
| 出力形式の追加 | `viewer_io.rs` の形式許可・エンコード・RGBA変換、保存失敗時の既存ファイル保持テスト |
| 画像送り・フォルダ移動 | `model.rs::Album`、`main.rs` の境界処理・見開きのページ送り、`pipeline.rs` の合成 |
| 補間・GPU画像の寿命 | `resample.rs`、`main.rs` の `clear_image` / `render` / `live_ids`、`ui_smoke.rs` |
| メモリー上限・キャッシュ容量 | `memory.rs` の `Budget::from_total` / `base_total` / `effective_total`、`pipeline.rs` の `Cache`、`main.rs` の `publish_live` / `respond_to_pressure` |
| GIFの再生・逐次デコード | `animation.rs` の `Decoder::push` / `wait_for_room` と `Player::tick`、`pipeline.rs` の `open_animation`、`main.rs` の `render`（`canvas`）、`viewer_io.rs` の `Frames` |

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

`cargo test` のPDFのテストは、合成PDFを補助プロセスで実際に描く。先に `just pdfium` を実行する。PDFiumがない場合はスキップせず失敗する。テストでは、テストバイナリーの `pdf::tests::helper_process` を補助プロセスとして起動し直す。テストは仲介スレッドを共有するため、先読み・表示解像度の種類を使うテストは `pdf::CLASS_TESTS` で順番に動かす（同じ種類の要求は最新の1件に置き換わる）。時間切れのテストは制限時間をテスト用の3秒にして確かめる。

テストは各モジュール内の `#[cfg(test)] mod tests` にある。実装上の理由と確認対象を対応させ、既存の回帰テストを維持する。

| 対象 | 主な検証内容 |
|---|---|
| `i18n` | Fluentの構文と重複、両言語のキー集合、日本語の変数が英語の引数に含まれること、全メッセージの整形、情報画面の桁区切りとファイルサイズの単位、OS言語の対応付け、単数・複数形、英語へのフォールバック、並行呼び出し、利用者の文字列の保持 |
| `menu_ui` | メニューに出すキーの選択（「次（Space）」より「次の画像」、既定の並び、利用者が変えたキー、修飾キーの少なさ）、キーの表示（修飾キー・矢印などの記号）、メニュー見出しと開く位置 |
| `model` | 数値順、空一覧、選択の維持、フォルダ探索、倍率とパン、PDFのページ列（文書順・並べ替えない）、見開きの先読み、PDFを入れ物とする探索（開けないPDFを飛ばす・隣接移動ではエラー）、PDFだけのフォルダ |
| `pdf` | fontconfigへの問い合わせ（Shift_JISの書体名・言語・明朝／ゴシック・太さ）、可変フォントと静的フォントの選択、TTCの書体と表、補助プロセスの制限（読み取り・プロセス生成・ソケットの拒否）、JPEGのスキャンの解像度、回転とCropBoxを反映した寸法、BGRA・白背景・`/Rotate`、画像主体の判定（不可視テキストを除く）と基準解像度、上限での縮小、指定倍率、暗号化・破損・0ページ・範囲外のページ、PDFiumを読み込めない補助プロセス、異常終了からの再起動と再試行の上限、制限時間と時間切れの記録・F5での消去、待つ人がいない長い描画の中断、メモリー上限、待つのをやめた要求、要求の優先順位と置き換え、フォームXObjectで包んだスキャン |
| `decoder` | BGRAと回転、JPE、拡張子と中身が違うファイル（WebP・PNG・GIF、別の拡張子のAVIF/HEIC）とftypのブランドの判定、破損ファイル、GIFの時間・回転・遅延0の保持・1フレームの上限・先頭フレームのみ・中止、実AVIF/HEIC |
| `animation` | 遅延の丸め、全フレーム保持（チャンクの大きさ、ループで再デコードしない）、予算超過時の逐次デコード（ループをまたぐ順序、先読みの上限、再生済みチャンクの退役）、保持の試みのペース制御、逼迫時の打ち切り、保持中からの逐次デコードへの移行、先頭前のデコード失敗 |
| `memory` | 予算の導出と下限・上限、OS情報で下げるだけの計算、meminfo・cgroupの解析と祖先の制限、台帳の重複除外、PSI通知の確認条件 |
| `pipeline` | PDFのページの基準解像度・先読みのキャッシュ・見開きの倍率合わせ（上限での縮小を含む）・逼迫中に縮めたページの描き直し、キャッシュ再利用とファイル更新、見開きの左右・横長・回転、幅0のページの拒否とワーカーの継続、Viewerの保持量に応じたキャッシュの縮小と重複の数え方、GIFの逐次デコード→全フレーム保持の登録→再訪時の再利用、見開きのGIFページとのキーの区別 |
| `resample` | PDFの見開きの描き直し（倍率と左右の並び）、古い結果の破棄、上限時のGPU復帰（全フレーム合計）、画像集合の一括補間と順序、最近傍、透明な縁、不透明・半透明の色とアルファ |
| `settings` | 保存・再読み込み、古いファイルの既定値補完、メモリー上限の正規化、破損JSON |
| `settings_ui` | メモリー上限の候補の循環、手書きの値の保持、表示 |
| `window_state` | 通常矩形の保持、最大化・全画面と再起動、画面変更・不正値・Waylandの位置制約 |
| `scrollbar` | バー位置とスクロール量、両端への移動 |
| `viewer_io` | 出力色、JPE、コーデック失敗時の既存ファイル保持、GIFの全フレーム保持からの書き出しと元ファイルの読み直しによる書き出し |

AVIF/HEICのテストは名前だけの疑似ファイルではなく、AV1/HEVCを実際にエンコード・デコードする。libheif本体と対応するデコーダーに加え、テストではエンコーダーも必要になる。

## 実ウィンドウの回帰テスト

描画・入力・設定の変更時は表示環境があれば実行する。既定はX11。`tools/check_ui.py` は子プロセスのWayland接続変数を外すため、X11へ接続できる環境が必要。ウィンドウ状態はWaylandでも検証できる。

```sh
cargo build --locked --features ui-smoke
python3 tools/check_ui.py
# 検証後は通常バイナリへ戻す（失敗した場合も同様）
cargo build --locked
```

スクリプトは一時ディレクトリに生成画像と設定を置き、ウィンドウのサイズ・位置（X11）・最大化・全画面と再起動、1,400回の画像送り、設定のキャンセル・保存・再起動、言語切替、セレクター、メニューのキー操作（左右で隣のメニュー、上下で区切り線を飛ばす）、キー変更、見開き・横長ページ・読む方向、補間後のアルファと色、正常終了を検証する。GIF（`NAGAME_UI_ANIMATION`）では、全フレーム保持の再生、全チャンクの最近傍補間、予算を小さくした逐次デコード（ループをまたぐ再生と保持量の上限）、全保持・逐次デコードそれぞれでGIFとPNGを混ぜた240回の画像送り、逼迫時の削減（`memory::signal_pressure`）を検証する。GIFはアプリ内で生成し、予算は `memory::override_budget` で差し替える。PDF（`NAGAME_UI_PDF`）では、合成した240ページのPDFで、文字のページのフィット（設定によらず拡大）、原寸より拡大したときの表示解像度での描き直し、200回のページ送り、倍率を揃えた見開き、2回の逼迫で補助プロセスを止めた後の回復、フォームXObjectで包んだスキャンの解像度、最終ページから同じフォルダの次のPDFへの移動を検証する。`check_ui.py` は `NAGAME_PDFIUM_LIBRARY` がなければ `tools/fetch_pdfium.py` で取得したPDFiumを渡す。実ウィンドウを描画するが、キーイベントは `ui_smoke.rs` がアプリ内へ配信する。OSからのマウス・DnD・外部アプリへの貼り付けは下記の手動回帰確認で扱う。

画像送りでのフォルダ境界移動と「前のフォルダ」／「前の同階層フォルダ」の直接操作も分けて検証する。先頭を開く設定のON/OFF、正順／逆順、自動移動／確認ダイアログの組み合わせで、画像送りは前フォルダの末尾・次フォルダの先頭、直接操作は設定通りの画像を選ぶことを確認する。

チェックボックスはSpaceによる下書きの切替とキャンセル、既存の見開き項目による保存・再起動後の復元を確認する。チェック部分と項目名のクリック、およびチェックあり／なしの見た目は手動でも確認する。

ウィンドウ状態だけを確認する場合は、`ui-smoke` ビルド後に以下を実行する。Waylandの検証には実際のWayland接続環境が必要。通常終了で保存→通常サイズの復元→最大化で終了→最大化の復元と解除を検証し、全画面で終了しても通常矩形を失わないことを確認する。

```sh
python3 tools/check_ui.py --window-state-only
python3 tools/check_ui.py --window-state-only --wayland
```

`ui-smoke` は専用featureであり、通常バイナリでは `NAGAME_UI_SMOKE` を設定しても起動しない。テストの一時設定を使うため、通常の利用者設定は変更しない。

`python3 tools/stress_navigation.py` はX11の外部入力を使う補助検証。libX11 / libXtst / xpropが必要で、専用ウィンドウだけに計1,400回の入力を送り、生存と移動方向を確認する。過去に成功した環境もあるが、直近の環境では合成入力が届かなかった。通常の回帰確認には `check_ui.py` を用い、外部入力の配送成功とは区別して扱う。

## 手動回帰確認

変更に応じて以下を実機で確認する。自動テストのアプリ内入力だけでは、OS経由の入力や外部アプリとの相互運用を保証しない。

| 対象 | 確認内容 |
|---|---|
| ファイル入力 | 選択ダイアログのキャンセル、画像／フォルダのDnD、日本語・スペースを含むパス、破損画像のエラー後の復帰 |
| 閲覧と描画 | 数値順と画像端の動作、フォルダ移動、ズーム・パンの制限、EXIF、回転4回、GIF切替、GIFの回転、大きなGIF（全保持の上限超え）の再生、全画面、自動送り、終了 |
| 非同期処理 | 読み込み・補間中の連続画像送りとズーム後に、最終選択の画像・倍率を表示すること |
| メニュー | メニューバーと右クリックメニューのクリック、✓・●の印とキー表示、画面端での右クリックメニューの位置 |
| 設定UI | 保存と再起動、キャンセル、キーの変更・解除、スクロールバーの実クリック／ドラッグ、固定位置の保存ボタン |
| セレクター | 候補のクリック決定、Esc／外側クリックで未確定候補を破棄、画面下部でも一覧が切れないこと |
| マンガ | 縦2枚→横1枚→縦2枚の前進・後退、RTL/LTR、横長単独表示、合成上限時の1ページ表示 |
| 表示補間 | 小さなピクセル画像の拡大と写真の縮小、最近傍とフィルターの差、不透明・半透明画像の色とアルファ |
| PDF | スキャンした本・文字のPDFの表示とページ送り、巻ごとのPDFの連続、見開き、拡大時の描き直し、非埋め込みの日本語、暗号化・破損PDFのエラー、情報画面、保存の既定名と画素数 |
| 外部出力 | 回転・見開き後の保存と再読込、GIFの時間（全保持・逐次デコード中の両方）、JPEの入出力、外部アプリへの画像／パス貼り付け、フォルダを開く操作 |

直近の実機確認では、X11の画像表示・画像送り・フォルダ移動・画像端の選択を確認済み。全項目を手動検証済みではなく、実マウスによるスクロールバー・セレクター、OSからのDnD、外部アプリへの画像貼り付けは未確認として残る。

## 性能調査

```sh
# GUIなし：ヘッダー取得、デコード・向き・BGRA変換の時間
cargo run --locked --bin decode-bench -- /path/to/image.jpg /path/to/image.png
# GUIあり：読み込み時間、キャッシュヒット、保持量・予算・PDFの補助プロセスのRSS・GIFの保持方式、逼迫への対応
NAGAME_PROFILE=1 ./target/debug/nagame /path/to/image.jpg
# 実ウィンドウ（ui-smokeビルド）：画像を順に開き、1秒ごとのRSS・CPU・保持量・予算・保持方式を出す
NAGAME_UI_SMOKE=1 NAGAME_UI_MEMORY_PROFILE='/path/a.gif:/path/b.png@10' ./target/debug/nagame /path/b.png
```

`@秒数` は1枚あたりの計測時間（既定6秒）。パスの代わりに `pressure` を書くと、表示中の画像のままメモリー逼迫を2回通知し、解放量を計測する。`check_ui.py` と同じく、X11で計測する場合は `WAYLAND_DISPLAY` を外して起動する。CPUは時間ベースのため、断続的なデコードではクロックが下がり実際の仕事量より大きく出る。X11の `import` で取ったウィンドウ画像はVulkanの表示内容より古い場合がある。描画の確認にはWaylandで起動してspectacle等で撮る。

直近の計測（Nobara、RAM 54 GiB、自動予算2 GiB、開発ビルド）：1280×720・150フレーム（展開527 MiB）のGIFは全フレーム保持でRSS約600 MiB、再生中のCPU約4%。1920×1080・160フレーム（1.27 GiB）は保持の試みが約1 GiBで逐次デコードへ移り、以後の保持量は約50〜100 MiB、デコードスレッドのCPUは約34%（10fps）。別の画像へ移るとRSSはすぐ下がった。

PDFの補助プロセスが選んだフォントと掛けた制限は、`NAGAME_PROFILE=1` で標準エラーへ出る（`pdf font: ...`・`pdf helper restricted: ...`）。

PDF（同じ環境、`check_ui.py` の合成PDF）：A4の文字ページ（150 DPIで1240×1754）と300 DPIのスキャン（600×800）を交互に並べた240ページで、200回のページ送りが約2.2秒。直後の本体のRSSは約110 MiB、補助プロセスのRSSは約150 MiB（PDFiumの文書・フォント・画像のキャッシュを含む）。

デコード計測にはGPUアップロードや描画時間を含まない。GUIログの `cached=true` はデコード済みキャッシュのヒットであり、`0 ms` はミリ秒単位の丸め。入力から画面表示まで0msという意味ではない。比較するときは同じ画像・ビルドプロファイル・コーデック環境で複数回測り、キャッシュの有無を分ける。

## 文書の更新

操作・制限が変わったらREADMEを更新する。処理の責務や不変条件が変わったら `architecture.md`、開発コマンド・検証方法が変わったらこの文書を更新する。

ソースのモジュールコメントと内部型の説明は以下でHTMLとして確認できる。

```sh
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps --document-private-items
```

通常バイナリの入口は `target/doc/nagame/index.html`。
