pub fn parse_cents(input: &str) -> Result<i64, String> {
    let amount: f64 = input.trim().parse().map_err(|_| "金额无效".to_owned())?;
    Ok((amount * 100.0) as i64)
}

pub fn format_cents(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, (cents % 100).abs())
}
