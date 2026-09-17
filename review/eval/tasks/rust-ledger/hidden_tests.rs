use eval_ledger::{amount::{format_cents, parse_cents}, csv::{parse_events, Event}, ledger::Ledger};
use std::io::Write;
use std::process::{Command, Stdio};

fn deposit(id: &str, account: &str, cents: i64) -> Event {
    Event::Deposit { id: id.into(), account: account.into(), cents }
}

fn transfer(id: &str, from: &str, to: &str, cents: i64) -> Event {
    Event::Transfer { id: id.into(), from: from.into(), to: to.into(), cents }
}

fn funded() -> Ledger {
    let mut ledger = Ledger::new();
    ledger.apply(&deposit("seed-a", "alice", 1000)).unwrap();
    ledger.apply(&deposit("seed-b", "bob", 200)).unwrap();
    ledger
}

#[test]
fn exact_decimal_and_full_range_roundtrip() {
    for (text, cents) in [("0.29", 29), ("1.15", 115), ("-0.01", -1), (" 12.3\t", 1230),
        ("0007.09", 709), ("-0", 0), ("92233720368547758.07", i64::MAX),
        ("-92233720368547758.08", i64::MIN)] {
        assert_eq!(parse_cents(text), Ok(cents), "{text}");
    }
    for cents in [i64::MIN, i64::MIN + 1, -1001, -100, -99, -1, 0, 1, 29, 115, i64::MAX] {
        let formatted = format_cents(cents);
        assert_eq!(parse_cents(&formatted), Ok(cents), "{formatted}");
        assert_eq!(formatted.split('.').last().unwrap().len(), 2);
        assert_eq!(formatted.starts_with('-'), cents < 0);
    }
    for cents in -999..=999 {
        assert_eq!(parse_cents(&format_cents(cents)), Ok(cents));
    }
}

#[test]
fn invalid_amount_grammar_and_overflow_are_errors() {
    for text in ["", " ", "+1", "--1", "1e2", "NaN", "inf", "-inf", ".1", "-.1", "1.",
        "0.001", "-0.001", "1.000", "1.2.3", "1 0", "1,00", "１２.３", "92233720368547758.08",
        "-92233720368547758.09", "99999999999999999999999999999999999999999999999999"] {
        assert!(parse_cents(text).is_err(), "accepted invalid amount {text:?}");
    }
}

#[test]
fn csv_normalizes_fields_and_preserves_physical_line_numbers() {
    let input = "  # merchant export\r\n\r\n deposit , id_1 , alice-1 , 0.29 \r\n\t\n";
    assert_eq!(parse_events(input).unwrap(), vec![deposit("id_1", "alice-1", 29)]);
    let error = parse_events("# first\n\n deposit,d1,alice,1\n  \ntransfer,bad,alice,bob,1,extra\n").unwrap_err();
    assert!(error.starts_with("第 5 行："), "{error}");
    assert!(parse_events("  # only comment\n \n").unwrap().is_empty());
}

#[test]
fn csv_rejects_bad_identifiers_nonpositive_amounts_and_extra_columns() {
    for input in ["deposit,,alice,1", "deposit,id,,1", "deposit,id,alice,0", "deposit,id,alice,-1",
        "deposit,id,a b,1", "deposit,id,用户,1", "deposit,i.d,alice,1", "deposit,id,alice,1,",
        "transfer,id,alice,bob,0", "transfer,id,alice,,1", "transfer,id,alice,bob,1 # comment",
        "account,balance", "DEPOSIT,id,alice,1", "deposit,\"id\",alice,1"] {
        let error = parse_events(input).expect_err(input);
        assert!(error.starts_with("第 1 行："), "{error}");
    }
}

#[test]
fn identical_replays_are_noops_and_conflicts_are_rejected() {
    let mut ledger = funded();
    let event = transfer("retry", "alice", "bob", 115);
    assert_eq!(ledger.apply(&event), Ok(true));
    for _ in 0..3 { assert_eq!(ledger.apply(&event), Ok(false)); }
    let snapshot = ledger.balances().clone();
    for conflict in [transfer("retry", "alice", "bob", 116),
        transfer("retry", "bob", "alice", 115), deposit("retry", "alice", 115),
        deposit("seed-a", "other", 1000)] {
        assert!(ledger.apply(&conflict).is_err(), "accepted conflict {conflict:?}");
        assert_eq!(ledger.balances(), &snapshot);
    }
    assert_eq!(ledger.balance("alice"), Some(885));
    assert_eq!(ledger.balance("bob"), Some(315));
    assert_eq!(ledger.apply(&deposit("seed-a", "alice", 1000)), Ok(false));
}

#[test]
fn missing_target_failure_does_not_debit_or_consume_request_id() {
    let mut ledger = funded();
    let event = transfer("late", "alice", "carol", 29);
    let snapshot = ledger.balances().clone();
    assert!(ledger.apply(&event).is_err());
    assert_eq!(ledger.balances(), &snapshot);
    assert_eq!(ledger.balance("carol"), None);
    ledger.apply(&deposit("seed-c", "carol", 1)).unwrap();
    assert_eq!(ledger.apply(&event), Ok(true));
    assert_eq!(ledger.balance("alice"), Some(971));
    assert_eq!(ledger.balance("carol"), Some(30));
    assert_eq!(ledger.apply(&event), Ok(false));
}

#[test]
fn insufficient_funds_failure_can_retry_after_funding() {
    let mut ledger = funded();
    let event = transfer("pending", "bob", "alice", 201);
    let snapshot = ledger.balances().clone();
    assert!(ledger.apply(&event).is_err());
    assert_eq!(ledger.balances(), &snapshot);
    ledger.apply(&deposit("topup", "bob", 1)).unwrap();
    assert_eq!(ledger.apply(&event), Ok(true));
    assert_eq!(ledger.balance("bob"), Some(0));
    assert_eq!(ledger.balance("alice"), Some(1201));
}

#[test]
fn direct_events_are_validated_without_mutating_state_or_ids() {
    let invalid = [deposit("", "alice", 1), deposit("valid", "", 1), deposit("bad.id", "alice", 1),
        deposit("valid", "a b", 1), deposit("valid", "用户", 1), deposit("valid", "alice", 0),
        deposit("valid", "alice", -1), deposit("valid", "alice", i64::MIN),
        transfer("valid", "alice", "alice", 1), transfer("valid", "alice", "bob", 0),
        transfer("valid", "alice", "bob", -1), transfer("valid", "alice", "bob", i64::MIN),
        transfer("valid", "missing", "bob", 1), transfer("", "alice", "bob", 1),
        transfer("bad.id", "alice", "bob", 1), transfer("valid", "alice", "b ob", 1)];
    for event in invalid {
        let mut ledger = funded();
        let snapshot = ledger.balances().clone();
        assert!(ledger.apply(&event).is_err(), "accepted {event:?}");
        assert_eq!(ledger.balances(), &snapshot, "mutated for {event:?}");
        if event.id() == "valid" {
            assert_eq!(ledger.apply(&deposit("valid", "carol", 1)), Ok(true));
        }
    }
}

#[test]
fn overflow_is_an_error_and_never_partially_commits() {
    let mut ledger = Ledger::new();
    ledger.apply(&deposit("max", "rich", i64::MAX)).unwrap();
    ledger.apply(&deposit("seed", "alice", 2)).unwrap();
    for event in [deposit("deposit-overflow", "rich", 1), transfer("transfer-overflow", "alice", "rich", 1)] {
        let snapshot = ledger.balances().clone();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ledger.apply(&event)));
        assert!(outcome.is_ok(), "overflow panicked");
        assert!(outcome.unwrap().is_err());
        assert_eq!(ledger.balances(), &snapshot);
        assert_eq!(ledger.apply(&deposit(event.id(), "carol", 1)), Ok(true));
    }
    assert_eq!(ledger.balance("rich"), Some(i64::MAX));
    assert_eq!(ledger.balance("alice"), Some(2));
}

fn cli(input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_eval_ledger"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn cli_integration_normalizes_replays_and_retains_user_customization() {
    let input = " # daily import\n deposit , d1 , zeta , 1.15\ndeposit,d2,alpha,0.29\n\n\
        transfer,t1,zeta,alpha,0.01\n transfer , t1 , zeta , alpha , 0.01\ndeposit,d1,zeta,1.15\n";
    let expected = "merchant_account,balance_cny\nalpha,0.30\nzeta,1.14\n";
    assert_eq!(eval_ledger::run(input), Ok(expected.to_owned()));
    let output = cli(input);
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    assert!(output.stderr.is_empty());
    assert_eq!(eval_ledger::run("\n# no rows\n"), Ok("merchant_account,balance_cny\n".into()));
}

#[test]
fn cli_any_import_error_fails_without_partial_stdout() {
    for input in ["deposit,d1,alice,10\ntransfer,t1,alice,missing,1\n",
        "deposit,d1,alice,10\ndeposit,d1,alice,11\n",
        "deposit,d1,alice,10\ndeposit,d2,bob,1\ntransfer,t1,bob,alice,2\n",
        "deposit,d1,alice,10\n\n# ignored\ndeposit,d2,bob,0.001\n",
        "deposit,d1,alice,92233720368547758.07\ndeposit,d2,alice,0.01\n"] {
        assert!(eval_ledger::run(input).is_err(), "library accepted {input:?}");
        let output = cli(input);
        assert!(!output.status.success(), "CLI accepted {input:?}");
        assert!(output.stdout.is_empty(), "partial stdout: {:?}", output.stdout);
        assert!(!output.stderr.is_empty());
    }
}
