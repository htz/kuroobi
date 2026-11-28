//! Writes a trained NNUE as the web model the browser loads: `nnue_to_web IN.bin OUT.bin`.

use std::path::Path;
use std::process::ExitCode;

use kuroobi::nnue::Nnue;
use kuroobi::pattern::NNUE_PATTERNS;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("usage: nnue_to_web IN.bin OUT.bin");
        return ExitCode::FAILURE;
    };
    let mut nn = Nnue::new(NNUE_PATTERNS);
    if let Err(e) = nn.load(Path::new(input)) {
        eprintln!("{input}: {e}");
        return ExitCode::FAILURE;
    }
    let mut bytes = Vec::new();
    nn.write_web(&mut bytes).expect("writing to memory");
    if let Err(e) = std::fs::write(output, &bytes) {
        eprintln!("{output}: {e}");
        return ExitCode::FAILURE;
    }
    println!("{output}: {} bytes", bytes.len());
    ExitCode::SUCCESS
}
