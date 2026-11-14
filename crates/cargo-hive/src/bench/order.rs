//! Fixed ABBA sample order: cold, seeded, seeded, cold, repeated.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Arm {
    Cold,
    Seeded,
}

pub fn abba(n: usize) -> Vec<(Arm, usize)> {
    let mut out = Vec::with_capacity(n.saturating_mul(2));
    for i in 0..n {
        if i % 2 == 0 {
            out.push((Arm::Cold, i));
            out.push((Arm::Seeded, i));
        } else {
            out.push((Arm::Seeded, i));
            out.push((Arm::Cold, i));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_alternate_cold_seeded_seeded_cold() {
        use Arm::*;
        assert_eq!(
            abba(2),
            vec![(Cold, 0), (Seeded, 0), (Seeded, 1), (Cold, 1)]
        );
        assert_eq!(
            abba(3),
            vec![
                (Cold, 0),
                (Seeded, 0),
                (Seeded, 1),
                (Cold, 1),
                (Cold, 2),
                (Seeded, 2)
            ]
        );
        assert_eq!(
            abba(4)[4..],
            [(Cold, 2), (Seeded, 2), (Seeded, 3), (Cold, 3)]
        );
    }
}
