pub mod amount;
pub mod csv;
pub mod ledger;
pub mod user_policy;

use amount::format_cents;
use csv::parse_events;
use ledger::Ledger;

pub fn run(input: &str) -> Result<String, String> {
    let mut ledger = Ledger::new();
    for event in parse_events(input)? {
        let _ = ledger.apply(&event);
    }
    let mut output = format!("{}\n", user_policy::REPORT_HEADER);
    for (account, cents) in ledger.balances() {
        output.push_str(&format!("{},{}\n", account, format_cents(*cents)));
    }
    Ok(output)
}
