use std::io::{self, Read};

fn main() {
    let mut input = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut input) {
        eprintln!("读取流水失败：{error}");
        std::process::exit(1);
    }
    match eval_ledger::run(&input) {
        Ok(output) => print!("{output}"),
        Err(error) => {
            eprintln!("导入失败：{error}");
            std::process::exit(1);
        }
    }
}
