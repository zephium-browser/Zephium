//! Summary statistics over a handful of repeated measurements.

pub(super) fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// The sample standard deviation; a single value has no spread to report.
pub(super) fn sd(values: &[f64]) -> Option<f64> {
    let mean = mean(values)?;
    if values.len() < 2 {
        return Some(0.0);
    }
    let squares: f64 = values.iter().map(|value| (value - mean).powi(2)).sum();
    Some((squares / (values.len() - 1) as f64).sqrt())
}

pub(super) fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_spread_and_median() {
        let values = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        assert_eq!(mean(&values), Some(5.0));
        assert!((sd(&values).unwrap() - 2.138_089_935).abs() < 1e-6);
        assert_eq!(median(&values), Some(4.5));
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
    }

    #[test]
    fn nothing_to_summarize_and_one_value() {
        assert_eq!(mean(&[]), None);
        assert_eq!(sd(&[]), None);
        assert_eq!(median(&[]), None);
        assert_eq!(sd(&[8.0]), Some(0.0));
    }
}
