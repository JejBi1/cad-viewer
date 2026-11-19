//! Number formatting of the viewer: a space as thousands separator and a comma as decimal separator
//! (Python `f"{v:,.{dec}f}"` with `,` -> space and `.` -> comma; main.py `fmt_len` / `fmt_num`).

/// `f"{v:,.{dec}f}".replace(",", " ").replace(".", ",")`. Negative zero keeps its sign as in Python; NaN and
/// infinities give `nan`, `inf`, `-inf`.
pub fn fmt_num(v: f64, dec: usize) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    let mut out = String::new();
    if v.is_sign_negative() {
        out.push('-');
    }
    let fixed = format!("{:.*}", dec, v.abs());
    let (int_part, frac_part) = match fixed.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (fixed.as_str(), None),
    };
    let n = int_part.len();
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    if let Some(f) = frac_part {
        out.push(',');
        out.push_str(f);
    }
    out
}

/// Length with two decimals (`fmt_len`), for example `1 234,50`.
pub fn fmt_len(v: f64) -> String {
    fmt_num(v, 2)
}

/// Python `f"{v:g}"`: six significant digits, trailing zeros removed, exponent form outside 1e-4 ... 1e6.
pub fn fmt_g(v: f64) -> String {
    const P: i32 = 6;
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    let sci = format!("{:.*e}", (P - 1) as usize, v);
    let (mant, exp) = match sci.split_once('e') {
        Some(parts) => parts,
        None => return sci,
    };
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..P).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{}{:02}", trim_zeros(mant), sign, exp.abs())
    } else {
        let decimals = (P - 1 - exp) as usize;
        trim_zeros(&format!("{:.*}", decimals, v))
    }
}

fn trim_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_len_matches_python() {
        // Python: f"{v:,.2f}".replace(",", " ").replace(".", ",")
        assert_eq!(fmt_len(0.0), "0,00");
        assert_eq!(fmt_len(1234.5), "1 234,50");
        assert_eq!(fmt_len(1234567.891), "1 234 567,89");
        assert_eq!(fmt_len(999.999), "1 000,00");
        assert_eq!(fmt_len(-1234.5), "-1 234,50");
        assert_eq!(fmt_len(0.005), "0,01");
        assert_eq!(fmt_len(0.015), "0,01"); // binary 0.015 is slightly below the tie
        assert_eq!(fmt_len(2.675), "2,67"); // binary 2.675 is slightly below the tie
    }

    #[test]
    fn fmt_num_zero_decimals_and_three_decimals() {
        assert_eq!(fmt_num(1234567.0, 0), "1 234 567");
        assert_eq!(fmt_num(999.0, 0), "999");
        assert_eq!(fmt_num(2.5, 0), "2"); // round half to even, as Python
        assert_eq!(fmt_num(3.5, 0), "4");
        assert_eq!(fmt_num(12.3456, 3), "12,346");
        assert_eq!(fmt_num(100.0, 3), "100,000");
        assert_eq!(fmt_num(1000.0, 1), "1 000,0");
    }

    #[test]
    fn fmt_num_signed_zero_and_non_finite() {
        // Python: f"{-0.0:,.2f}" == "-0.00", f"{-0.001:,.2f}" == "-0.00"
        assert_eq!(fmt_num(-0.0, 2), "-0,00");
        assert_eq!(fmt_num(-0.001, 2), "-0,00");
        assert_eq!(fmt_num(f64::NAN, 2), "nan");
        assert_eq!(fmt_num(f64::INFINITY, 2), "inf");
        assert_eq!(fmt_num(f64::NEG_INFINITY, 0), "-inf");
    }

    #[test]
    fn fmt_g_matches_python() {
        // Python: f"{v:g}"
        assert_eq!(fmt_g(7.85), "7.85");
        assert_eq!(fmt_g(8.0), "8");
        assert_eq!(fmt_g(0.001), "0.001");
        assert_eq!(fmt_g(12.345678), "12.3457");
        assert_eq!(fmt_g(30.0), "30");
        assert_eq!(fmt_g(0.0001), "0.0001");
        assert_eq!(fmt_g(0.00001), "1e-05");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(0.0), "0");
    }
}
