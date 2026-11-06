#[path = "../decoder.rs"]
mod decoder;
// The benchmark shares decoder errors, but does not use the UI localization helpers.
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
