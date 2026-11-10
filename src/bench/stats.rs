//! Integer median, p90, min, max and spread for a series of sample times.

pub struct Summary {
    pub median: u64,
    pub p90: u64,
    pub min: u64,
    pub max: u64,
    pub spread: u64,
}

pub fn summarize(values: &[u64]) -> Option<Summary> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        let a = sorted[n / 2 - 1];
        let b = sorted[n / 2];
        a / 2 + b / 2 + (a % 2 + b % 2) / 2
    };
    let p90 = sorted[(9 * n).div_ceil(10) - 1];
    let min = sorted[0];
    let max = sorted[n - 1];
    Some(Summary {
        median,
        p90,
        min,
        max,
        spread: max - min,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_series_summarizes_by_nearest_rank_in_integers() {
        for (values, median, p90) in [
            (vec![5], 5, 5),
            (vec![10, 30], 20, 30),
            (vec![10, 31], 20, 31),
            (vec![3, 1, 2], 2, 3),
            ((1..=9).collect::<Vec<u64>>(), 5, 9),
            ((1..=10).collect::<Vec<u64>>(), 5, 9),
            ((1..=11).collect::<Vec<u64>>(), 6, 10),
        ] {
            let s = summarize(&values).unwrap();
            assert_eq!((s.median, s.p90), (median, p90), "{values:?}");
            assert_eq!(s.spread, s.max - s.min);
        }
        assert!(summarize(&[]).is_none());
    }

    #[test]
    fn the_mean_of_two_huge_middles_does_not_overflow() {
        assert_eq!(
            summarize(&[u64::MAX - 1, u64::MAX]).unwrap().median,
            u64::MAX - 1
        );
    }
}
