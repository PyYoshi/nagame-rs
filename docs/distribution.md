# 配布用ビルドとAppImage

## 対象と環境

最初の配布対象はLinux x86_64。ビルドの窓口は [cross](https://github.com/cross-rs/cross)、コンテナーエンジンはPodman（PodmanがなくDockerがある場合はDocker）とする。[Cross.toml](../Cross.toml) から [Containerfile](../packaging/linux/Containerfile) を指定し、GPUIとlibheifに必要なネイティブライブラリを用意する。crossはコンテナーのビルド、Rustツールチェーンのマウント、Cargoの実行を担当する。[公式のカスタムイメージ設定](https://github.com/cross-rs/cross/wiki/Configuration#custom-images) を利用している。

| 項目 | 選択 |
|---|---|
| ビルドOS | Ubuntu 24.04 amd64。ベースイメージのdigestをContainerfileで固定 |
| glibc | 2.39 |
| Rust | 1.99.1。justfileの `cross +1.99.1` で指定 |
| Rustターゲット | `x86_64-unknown-linux-gnu` |
| CPU | `target-cpu=x86-64`。開発機固有の `native` は使わない |
| libheif | Ubuntu 24.04の1.17.6系。現在のRust側の最小要件1.17を満たす |
| cross | aquaで0.2.5を固定 |

Ubuntu 24.04は [標準セキュリティ保守が2029年5月まで](https://ubuntu.com/about/release-cycle) 続き、必要な [libheif-dev](https://packages.ubuntu.com/noble/libheif-dev) を標準リポジトリから導入できる。開発機の新しいglibcへリンクすることを避け、この環境を配布用の基準とする。

現状はx86_64 Linuxホストでのビルドを対象とする。別CPU向けには、そのCPU用のC/C++ツールチェーンとネイティブライブラリを含むイメージの追加が必要で、crossのターゲット名を変えるだけでは対応しない。

## 実行

ホストにrustupとPodmanまたはDockerを用意し、`podman info`（Dockerなら `docker info`）が成功する状態にする。開発用CLIの導入は [開発・検証](development.md#開発ツールと秘密情報の検査) を参照する。初回はネットワーク接続が必要。

```sh
aqua policy allow aqua-policy.yaml
aqua install
just dist-build
just dist-test
just dist-appimage
# AppImage生成と隔離環境での起動・読み込み確認まで行う場合
just dist-check-appimage
# デスクトップ環境がある場合、生成済みAppImageを実ドライバーでも確認
just dist-check-appimage-host
```

justfileは `CROSS_CONTAINER_ENGINE` を見てエンジンを選ぶ。未設定ならPodmanを使い、PodmanがなくDockerがある場合だけDockerを使う。両方ある環境でDockerを使うときは `CROSS_CONTAINER_ENGINE=docker just dist-appimage` のように指定する。選んだエンジンは、crossと、AppImageを組み立てるコンテナー・確認用のコンテナーに共通で使う。イメージはエンジンごとに保存されるため、切り替えた直後はイメージを作り直す。

AppDirの組み立ては、`apt-get source` のためにコンテナー内のrootで行う。rootのコンテナーエンジンでは、作ったファイルの所有者がホストのrootになる。このため組み立て後に、所有者を利用者へ戻す（失敗時も）。rootかどうかの判定はcrossと同じで、Docker（Podmanの互換コマンドを除く）をroot、ほかをrootlessとみなす。rootlessのDockerなどでは `CROSS_ROOTLESS_CONTAINER_ENGINE=1`（逆は `0`）で指定する。

`aqua policy allow` の前に [aqua-policy.yaml](../aqua-policy.yaml) と [ローカルレジストリ](../packaging/linux/aqua-registry.yaml) を確認する。aquaが標準外レジストリを既定で拒否するための初回設定で、標準レジストリに加え、指定した4ツールだけを許可する。[aquaのポリシー仕様](https://aquaproj.github.io/docs/guides/policy-as-code/) を参照。

crossがカスタムイメージを作成し、必要に応じて指定版のRustをrustupで導入する。ホスト側のシステムライブラリの導入は不要。Cargoのレジストリキャッシュとソースをコンテナーにマウントして使い、成果物は通常の開発ビルドと分けて `target/dist-linux-x86_64/` に出力する。ソースの絶対パスはRustの `--remap-path-prefix` で置換する。

配布用バイナリの出力先:

```text
target/dist-linux-x86_64/x86_64-unknown-linux-gnu/release/nagame
```

`just release` はホスト環境でのリリースビルドであり、配布用とは区別する。環境を更新するときはContainerfileのdigest、justfileのRust版、aquaのcross版とチェックサムを明示的に更新し、ビルドとテストを再実行する。aptパッケージはUbuntuの更新を取り込むため、ベースのdigest固定だけでビット単位の再現性を保証する構成ではない。

## AppImageの構成と成果物

`just dist-appimage` は配布用バイナリをビルドし、秘密情報を検査してから [tools/package_appimage.py](../tools/package_appimage.py) を実行する。Ubuntu内の [package.py](../packaging/linux/package.py) がlinuxdeployで共有ライブラリを収集し、appimagetoolでAppImageを生成する。複数段階の収集・検証を行うためPythonで管理し、単純なビルドコマンドはjustfileから直接実行する。

| ツール | 固定版 |
|---|---|
| linuxdeploy | 1-alpha-20251107-1 |
| appimagetool | 1.9.1 |
| type2-runtime | 20251108 |
| cargo-about | 0.9.3 |

ツール本体・実行ランタイムはaquaでチェックサム照合する。`aqua which` はパスを返すだけで導入しないため、[package_appimage.py](../tools/package_appimage.py) は先に `aqua install` を実行してからパスを取る。コンテナーへ渡すツール・バイナリー・ディレクトリーは、マウント前に実在を確かめる。Dockerの `-v` は存在しないホストのパスをrootのディレクトリーとして作り、コンテナー内での実行が `Permission denied` で失敗するため。appimagetoolには `--runtime-file` を渡し、最新ランタイムの自動取得を避ける。[公式のオプション説明](https://github.com/AppImage/appimagetool#usage) を参照。FUSEのないコンテナーでは生成ツールを展開して実行する。

`target/dist/appimage/` に次を生成する。ファイル名の版はCargo.tomlに従う。

| ファイル | 内容 |
|---|---|
| `Nagame-0.1.0-x86_64.AppImage` | 実行可能なAppImage |
| `Nagame-0.1.0-source.tar.gz` | 作業ツリーのスナップショット、Rust依存、同梱ネイティブライブラリのUbuntuソース、ランタイムのソース・再リンク用入力、ライセンス |
| `build-info.json` | 元コミット、未コミット変更の有無、元バイナリとランタイムのSHA256、PDFiumの版・URL・SHA256、同梱ライブラリとソースパッケージの版 |
| `SHA256SUMS` | 上記3ファイルのSHA256 |

AppDirにはPNGアイコン・desktopファイル・独自のAppRunと、`usr/share/doc/nagame/` のライセンス一覧を含む。ライブラリはUbuntuのものを収集し、AVIFには `libheif-aomdec.so`、HEICには `libheif-libde265.so` を同梱する。`LIBHEIF_PLUGIN_PATH` をAppDir内へ向け、ホスト側やエンコーダーのプラグインを参照しない。PDFiumはUbuntuのパッケージではなく、[pdfium.json](../packaging/linux/pdfium.json) で固定した配布物（`tools/fetch_pdfium.py` がSHA256を照合して取得）を `usr/lib/libpdfium.so` に置く。PDFの補助プロセスが実行ファイルから見た `../lib/libpdfium.so` を読み込む。依存はglibcとlibgcc_sだけ。ライセンスは `usr/share/doc/nagame/third-party/pdfium/` にPDFium本体と組み込みの第三者分（FreeType・ICU・libjpeg-turbo・OpenJPEG・LCMS・abseil等）を含める。glibc・libgcc・libstdc++・Waylandクライアント／カーソルライブラリ・VulkanローダーやGPUドライバーは同梱しない。実行環境にはglibc 2.39以降と対応するC++ランタイム、X11またはWayland、Vulkan環境、フォントが必要。Waylandで起動する場合はホストの `libwayland-client.so.0` と `libwayland-cursor.so.0` を使う。

WaylandライブラリはホストのGPUドライバーと揃える。古い `libwayland-client` を同梱すると `LD_LIBRARY_PATH` で優先され、ホストの新しいMesaドライバーが必要とするシンボルを提供できない場合がある。実際に `wl_fixes_interface` が解決できず、GPUIが `NoSupportedDeviceFound` で停止したため、linuxdeployの `--exclude-library libwayland-*.so*` で明示的に除外している。ライブラリ構成を変更する際は、古いUbuntu内の描画テストに加え、新しいホストのドライバーでも検証する。

```sh
chmod +x Nagame-0.1.0-x86_64.AppImage
./Nagame-0.1.0-x86_64.AppImage /path/to/image.png
# FUSEを使えない場合
./Nagame-0.1.0-x86_64.AppImage --appimage-extract-and-run /path/to/image.png
```

### アプリメニューとウィンドウのアイコン

Waylandでは、GPUIのウィンドウが送る `app_id=nagame` と、ホストにインストールされた `nagame.desktop` の対応からアイコンを解決する。AppImage内にdesktopファイルとアイコンがあっても、ホストのデスクトップ環境へ自動登録されるわけではない。未登録ではWaylandの汎用アイコンになる場合がある。[xdg-shellのapp_id仕様](https://wayland.app/protocols/xdg-shell#xdg_toplevel:request:set_app_id) を参照。

AppImageを置く場所を決めたら、プロジェクトルートから次を実行する。通常の `nagame` バイナリも同じ方法で登録できる。

```sh
just install-desktop /path/to/Nagame-0.1.0-x86_64.AppImage
```

[install_desktop.py](../tools/install_desktop.py) が `$XDG_DATA_HOME/applications/nagame.desktop` と `$XDG_DATA_HOME/icons/hicolor/` のPNG・SVGを登録する。`XDG_DATA_HOME` が空または未設定なら `~/.local/share` を使う。desktopファイルの起動先は指定した実行ファイルの絶対パス、`StartupWMClass` は `nagame`。`Icon` は登録したSVGの絶対パスを指定し、起動中のデスクトップにテーマ検索の失敗結果が残っている場合にも影響されないようにする。個人環境の絶対パスは生成ファイルだけに記録し、リポジトリには含めない。

登録時に、利用できるアイコン・desktopキャッシュ更新コマンドを実行する。D-Busセッションがある場合は [KIconLoaderと同じアイコン更新通知](https://github.com/KDE/kiconthemes/blob/master/src/kiconloader.cpp) も送り、起動中のPlasmaのキャッシュを更新する。アイコン名を検索するコマンドの成功だけでは、既存のPlasmaプロセスでの表示まで確認できたことにはならない。

登録後は、開いているNagameを閉じて起動し直す。アプリメニューからの起動も利用できる。バイナリを移動した場合は、新しいパスを指定して再登録する。画像形式の既定アプリは変更しない。登録を取り消す場合は、上記desktopファイルと `hicolor/{32x32,64x64,256x256,512x512}/apps/nagame.png`・`hicolor/scalable/apps/nagame.svg` を削除する。

## ライセンスとソースの同時提供

Nagame独自のコード・素材は [GPL-3.0-or-later](../LICENSE)。第三者のコードをこのライセンスへ変更するものではない。Rust依存のライセンス本文はcargo-about、同梱ネイティブライブラリは実際のファイルとdpkg所有パッケージを照合して収集する。ライセンスが解決できない、元パッケージや対応するソースを取得できない場合は生成を失敗させる。

ソースアーカイブには `cargo vendor --locked` で集めたRust依存と相対パスのCargo設定を含む。Ubuntuのネイティブライブラリは同じ版の `.dsc`・元ソース・Debianパッチを `apt-get source --download-only` で収集する。`vendor/xattr` の修正とMIT/Apacheライセンスも保持する。

PDFiumはBSD-3-Clause／Apache-2.0で、ソースの提供義務はない。Ubuntuのソースパッケージの収集（dpkgの所有パッケージの照合）からは除き、版とSHA256を `build-info.json` に、差し替え方法を `SOURCE.md` に記録する。

[libheifはLGPL-3.0](https://github.com/strukturag/libheif/blob/master/COPYING)。動的リンクのまま同梱する。AppImageを展開すると共有ライブラリを交換してAppRunから実行できる。実行ランタイムには静的なlibfuseも含まれるため、ランタイムのソース・libfuseの入力とパッチ・再リンク手順も提供する。[ランタイム側のライセンス](https://github.com/AppImage/type2-runtime/blob/20251108/LICENSE) を参照。ランタイム用ソース入力のURLとSHA256は [runtime-sources.json](../packaging/linux/runtime-sources.json) に固定している。上流バイナリに使われた全Alpineパッケージの厳密な版や、ビット単位の再現性を主張するものではない。

公開時はAppImageだけでなく、対応するソースアーカイブ・build-info.json・SHA256SUMSを同じリリースに置く。アーカイブ内の [SOURCE.md](../packaging/linux/SOURCE.md) にオフラインのCargoビルドとネイティブライブラリ・ランタイムの差し替え方法を記載している。未コミット状態もローカル検証に使えるが、公開用はコミットを確定して生成し、`build-info.json` の `dirty` が `false` であることを確認する。

## 確認済みの範囲

2026-11-07にx86_64 Linuxホストから `just dist-build` と `just dist-test` を実行し、リリースビルドと単体テスト24件が成功した。AVIF/HEICの実コーデックのテストも含む。生成したバイナリの `--help` は同じUbuntuコンテナー内で実行できた。

PDF対応の組み込み後（同日）に、`just dist-test` でPDFの描画テスト（コンテナー内で補助プロセスを起動し、取得済みのPDFiumで合成PDFを描く）を含む単体テストが成功した。`just dist-check-appimage` と `just dist-check-appimage-host` では、同梱した `libpdfium.so` とライセンスの配置、隔離環境（PDFiumなし）とホストのWayland・X11での合成PDFの読み込みを確認した。

段階2（fontconfigによる代替フォント、補助プロセスのLandlock・seccomp）の組み込み後も、`just dist-test`（コンテナー内で制限を掛けた補助プロセスの確認を含む）、`just dist-check-appimage`、`just dist-check-appimage-host` が成功した。

`readelf --version-info` でバイナリが要求するGLIBCシンボルの最大版は2.39だった。バイナリに開発ディレクトリの絶対パスが含まれないことも確認した。

AppImageは [Smoke.Containerfile](../packaging/linux/Smoke.Containerfile) のUbuntu 24.04内で確認する。この環境にはシステムのlibheifやプラグインを導入しない。`just dist-check-appimage` は既存の実コーデックテストから8/10bit AVIF/HEICを生成し、AppImageのSHA256、展開・FUSE不要モード、同梱プラグインの構成、Xvfb＋Mesa VulkanでのGUI起動とPNG・AVIF/HEIC・PDF（スクリプトが生成する1ページの合成PDF）、拡張子と中身が違うファイル（中身がPNGの `.avif`、中身がAVIFの `.png`）の読み込み完了を検証する。ピクセル比較・入力操作の回帰テストではなく、配布物の起動とデコードの検証。

`just dist-check-appimage-host` は生成済みのAppImageとコーデック検証用画像を使い、利用できるWaylandとX11の両方で、通常のAppImage実行経路（FUSE）からGUI起動と読み込みを確認する。ホストの設定は変更せず、一時的な設定ディレクトリを使う。同梱Waylandライブラリの再混入とVulkanドライバーのシンボル解決エラーも検出する。

## 公開前に残る作業

他ディストリビューションと異なるGPU／ドライバーでの互換性確認、GitHub Actionsでの生成とReleasesへの公開は別途行う。glibcの要求だけでは、GPUドライバーや実行時ライブラリを含む動作互換性は保証できない。[AppImageの推奨事項](https://docs.appimage.org/reference/best-practices.html) に従い、対応範囲の下限で動作確認する。
