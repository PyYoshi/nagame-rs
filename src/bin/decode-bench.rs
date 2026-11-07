//! GUIを起動せずに、ヘッダーの読み取りと全フレームのデコードにかかる時間を測る。
//! `cargo run --locked --bin decode-bench -- 画像...`。デコードに失敗したら、残りの画像は
//! 測らずに終了コード1で終わる。
#[path = "../decoder.rs"]
mod decoder;
// ベンチマークはデコーダーのエラー文を共有するが、UI向けの翻訳ヘルパーは使わない。
#[allow(dead_code)]
#[path = "../i18n.rs"]
mod i18n;

fn main() {
    for path in std::env::args_os().skip(1) {
        let probe = std::time::Instant::now();
        let header = decoder::dimensions(std::path::Path::new(&path));
        eprintln!("header {:?}: {:?} in {:?}", path, header, probe.elapsed());
        let start = std::time::Instant::now();
        match decoder::decode(std::path::Path::new(&path), 0) {
            Ok(image) => println!(
                "{:?}: {:?}, {:?}, {} frames",
                path,
                start.elapsed(),
                image.dimensions,
                image.frames.len()
            ),
            Err(error) => {
                eprintln!("{:?}: {error}", path);
                std::process::exit(1);
            }
        }
    }
}
