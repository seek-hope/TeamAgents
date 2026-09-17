use eval_ledger::{amount, csv, ledger::Ledger};

#[test]
fn whole_yuan_amount() {
    assert_eq!(amount::parse_cents("12.00").unwrap(), 1200);
    assert_eq!(amount::format_cents(1200), "12.00");
}

#[test]
fn clean_deposit_and_transfer() {
    let events = csv::parse_events("deposit,d1,alice,10.00\ndeposit,d2,bob,2.00\ntransfer,t1,alice,bob,3.00\n").unwrap();
    let mut ledger = Ledger::new();
    for event in events { assert!(ledger.apply(&event).unwrap()); }
    assert_eq!(ledger.balance("alice"), Some(700));
    assert_eq!(ledger.balance("bob"), Some(500));
}

#[test]
fn report_retains_user_header() {
    assert_eq!(eval_ledger::run("deposit,d1,alice,10.00\n").unwrap(),
        "merchant_account,balance_cny\nalice,10.00\n");
}
