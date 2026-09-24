use crate::amount::parse_cents;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Deposit { id: String, account: String, cents: i64 },
    Transfer { id: String, from: String, to: String, cents: i64 },
}

impl Event {
    pub fn id(&self) -> &str {
        match self {
            Self::Deposit { id, .. } | Self::Transfer { id, .. } => id,
        }
    }
}

pub fn parse_events(input: &str) -> Result<Vec<Event>, String> {
    input.lines().filter(|line| !line.is_empty() && !line.starts_with('#'))
        .enumerate().map(|(index, line)| {
            let fields: Vec<_> = line.split(',').collect();
            let result = match fields.as_slice() {
                ["deposit", id, account, amount] => parse_cents(amount).map(|cents| Event::Deposit {
                    id: (*id).to_owned(), account: (*account).to_owned(), cents,
                }),
                ["transfer", id, from, to, amount] => parse_cents(amount).map(|cents| Event::Transfer {
                    id: (*id).to_owned(), from: (*from).to_owned(), to: (*to).to_owned(), cents,
                }),
                _ => Err("流水格式无效".to_owned()),
            };
            result.map_err(|error| format!("第 {} 行：{}", index + 1, error))
        }).collect()
}
