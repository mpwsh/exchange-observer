pub fn get_percentage_diff(high: f64, low: f64) -> f32 {
    let range = high - low;
    format!("{:.2}", (range / low) * 100.00).parse().unwrap()
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
