/// Multiply two numbers.
pub fn mul(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplies() {
        assert_eq!(mul(3, 4), 12);
    }
}
