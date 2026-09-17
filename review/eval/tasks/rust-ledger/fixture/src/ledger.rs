use std::collections::BTreeMap;
use crate::csv::Event;

#[derive(Debug, Default)]
pub struct Ledger {
    balances: BTreeMap<String, i64>,
    requests: BTreeMap<String, Event>,
}

impl Ledger {
    pub fn new() -> Self { Self::default() }

    pub fn balance(&self, account: &str) -> Option<i64> { self.balances.get(account).copied() }

    pub fn balances(&self) -> &BTreeMap<String, i64> { &self.balances }

    pub fn apply(&mut self, event: &Event) -> Result<bool, String> {
        if self.requests.contains_key(event.id()) { return Ok(false); }
        self.requests.insert(event.id().to_owned(), event.clone());
        match event {
            Event::Deposit { account, cents, .. } => {
                *self.balances.entry(account.clone()).or_default() += cents;
            }
            Event::Transfer { from, to, cents, .. } => {
                let source = self.balances.get_mut(from).ok_or("源账户不存在")?;
                if *source < *cents { return Err("余额不足".to_owned()); }
                *source -= cents;
                let target = self.balances.get_mut(to).ok_or("目标账户不存在")?;
                *target += cents;
            }
        }
        Ok(true)
    }
}
