#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: fixtures <out dir>"));
    std::fs::create_dir_all(&out).unwrap();
    for dir in ["golden", "golden-extra"] {
        for case in common::cases(dir) {
            let name = case.file_name().unwrap().to_string_lossy().to_string();
            std::fs::write(out.join(format!("{name}.sigdb")), common::case_bytes(&case)).unwrap();
            println!("{dir}/{name}");
        }
    }
}
