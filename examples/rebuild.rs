#[path = "../tests/common/mod.rs"]
mod common;

use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let (input, output) = (args.next().expect("build.json"), args.next().expect("out.sigdb"));
    let t = Instant::now();
    let build = common::read_json(std::path::Path::new(&input));
    let (rules, options) = common::options_from(&build).unwrap();
    let parsed = t.elapsed();
    let t = Instant::now();
    let result = sigdb::build(rules.as_ref(), &output, &options).unwrap();
    println!(
        "read {:.2}s, build {:.2}s, {} bytes, {} sections",
        parsed.as_secs_f64(),
        t.elapsed().as_secs_f64(),
        result.size,
        result.sections.len()
    );
}
