//! `cargo bench -p matty-markdown --bench parse`: parses a 10 KB message of
//! mixed Markdown and fails if the median of 200 runs passes 1 ms (#41).

use std::time::{Duration, Instant};

const BUDGET: Duration = Duration::from_millis(1);
const RUNS: usize = 200;

fn message() -> String {
    let chunk = "Hey @ana, the **build** on ~town-square is _green_ again :tada:. \
                 See https://ci.example.com/runs/1234?tab=logs and `cargo test`.\n\n\
                 - first item with a [link](https://example.com)\n\
                 - second ~~old~~ item\n  - nested #release-notes\n\n\
                 > quoted line\n\n\
                 ```rust\nfn main() { println!(\"hi\"); }\n```\n\n\
                 | a | b |\n|---|---|\n| 1 | 2 |\n\n";
    let mut text = String::new();
    while text.len() < 10 * 1024 {
        text.push_str(chunk);
    }
    text
}

fn main() {
    let text = message();
    let mut times: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let started = Instant::now();
            let document = matty_markdown::parse(&text);
            let elapsed = started.elapsed();
            std::hint::black_box(document);
            elapsed
        })
        .collect();
    times.sort();
    let median = times[RUNS / 2];
    eprintln!("parse {} bytes: median {median:?}", text.len());
    if median > BUDGET {
        eprintln!("over the {BUDGET:?} budget");
        std::process::exit(1);
    }
}
