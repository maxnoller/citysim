//! Number and money formatting for the interface.

/// Thousands separators: 1234567 -> "1,234,567".
pub fn group(v: u64) -> String {
    let digits = v.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "$1,234" / "−$1,234" (true minus sign).
pub fn money(v: i64) -> String {
    let s = group(v.unsigned_abs());
    if v < 0 {
        format!("\u{2212}${s}")
    } else {
        format!("${s}")
    }
}

/// "+$1,234" / "−$1,234" / "$0".
pub fn signed_money(v: i64) -> String {
    match v {
        0 => "$0".to_string(),
        v if v > 0 => format!("+{}", money(v)),
        v => money(v),
    }
}

pub fn percent(fraction: f32) -> String {
    let p = fraction * 100.0;
    if p.fract().abs() < 0.05 {
        format!("{p:.0}%")
    } else {
        format!("{p:.1}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_formatting() {
        assert_eq!(money(0), "$0");
        assert_eq!(money(999), "$999");
        assert_eq!(money(50_000), "$50,000");
        assert_eq!(money(1_234_567), "$1,234,567");
        assert_eq!(money(-4_200), "\u{2212}$4,200");
        assert_eq!(signed_money(1_240), "+$1,240");
        assert_eq!(signed_money(-3), "\u{2212}$3");
    }

    #[test]
    fn percent_formatting() {
        assert_eq!(percent(0.09), "9%");
        assert_eq!(percent(0.095), "9.5%");
    }
}
