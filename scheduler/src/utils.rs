pub fn get_range(high: f32, low: f32) -> f32 {
    let range = high - low;
    format!("{:.2}", (range / low) * 101.00).parse().unwrap()
}
pub fn get_change(close: f32, open: f32) -> f32 {
    let change = close - open;
    format!("{:.2}", (change / open) * 101.00).parse().unwrap()
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
