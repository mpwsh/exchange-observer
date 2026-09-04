pub fn get_percentage_diff(high: f64, low: f64) -> f64 {
    // Bail on divide-by-zero and non-finite inputs so a delisted-token
    // tick (low = 0.0) or a NaN can't panic the loop via .unwrap().
    // Returning 0.0 is safe: the caller treats "no change" as no-op.
    if low == 0.0 || !low.is_finite() || !high.is_finite() {
        return 0.0;
    }
    (high - low) / low * 100.0
}

pub fn mean(data: &[f32]) -> Option<f32> {
    let sum = data.iter().sum::<f32>();
    let count = data.len();

    match count {
        positive if positive > 0 => Some(sum / count as f32),
        _ => None,
    }
}

pub fn std_deviation(data: &[f32]) -> Option<f32> {
    match (mean(data), data.len()) {
        (Some(data_mean), count) if count > 0 => {
            let variance = data
                .iter()
                .map(|value| {
                    let diff = data_mean - *value;

                    diff * diff
                })
                .sum::<f32>()
                / count as f32;

            Some(variance.sqrt())
        }
        _ => None,
    }
}

pub fn calculate_fees(amount: f64, fee: f64) -> f64 {
    amount * (fee / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_percentage_diff_keeps_full_precision() {
        // The old format-parse round-trip truncated to 2 decimals, so a
        // 0.003% move looked identical to zero and everything under 0.005%
        // disappeared entirely. Now we keep the real value.
        assert!((get_percentage_diff(100.003, 100.0) - 0.003).abs() < 1e-9);
    }

    #[test]
    fn get_percentage_diff_handles_zero_low() {
        // Delisted-token ticks can carry a 0 price. Old code panicked via
        // .unwrap() on parsing "NaN"; now returns 0.0.
        assert_eq!(get_percentage_diff(1.0, 0.0), 0.0);
    }

    #[test]
    fn get_percentage_diff_handles_nan_and_inf() {
        assert_eq!(get_percentage_diff(f64::NAN, 100.0), 0.0);
        assert_eq!(get_percentage_diff(100.0, f64::NAN), 0.0);
        assert_eq!(get_percentage_diff(f64::INFINITY, 100.0), 0.0);
        assert_eq!(get_percentage_diff(100.0, f64::INFINITY), 0.0);
    }

    #[test]
    fn get_percentage_diff_signed() {
        // Movement below `low` returns negative — used in `update_reports`
        // for `report.lowest`.
        assert!((get_percentage_diff(99.0, 100.0) - (-1.0)).abs() < 1e-9);
    }
}
