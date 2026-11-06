//! One fixed sample, timed with `std::time`. This is not a comparison with IDEA.
use java_engine::{ParsedFile, TextEdit};
use std::time::Instant;

#[test]
fn parse_and_edit_baseline_on_one_sample() {
    let source = sample();
    let parses = 20u32;
    let started = Instant::now();
    let mut file = ParsedFile::parse(&source);
    for _ in 1..parses {
        file = ParsedFile::parse(&source);
    }
    let parse_us = started.elapsed().as_micros();
    let at = source.find("SEED").unwrap() as u32;
    let edit_started = Instant::now();
    let edited = file.apply_edit(TextEdit { start_byte: at, end_byte: at + 4, insert: "NEXT".into() });
    let edit_us = edit_started.elapsed().as_micros();
    println!(
        "baseline host=this-process bytes={} parses={parses} parse_total_us={parse_us} one_edit_us={edit_us} mode={:?}",
        source.len(),
        edited.mode
    );
    assert_eq!(edited.file.root().as_ref(), ParsedFile::parse(edited.file.text()).root().as_ref());
}

fn sample() -> String {
    let mut out = String::from("class Sample {\n");
    for i in 0..40 {
        out.push_str(&format!(
            r#"
            int m{i}(int x) {{
                Object f = (int v) -> v + {i};
                String label = """
                    {{ brace {i} }}
                    """;
                Object ref = String::valueOf;
                int n = x + 1; // SEED
                return n;
            }}
            "#
        ));
    }
    out.push_str("}\n");
    out
}
