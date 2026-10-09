/// Turns successive values of an increasing counter into per-interval increases.
#[derive(Debug, Clone, Default)]
pub(crate) struct Counter {
    last: Option<u64>,
    wrap_at: Option<u64>,
}

impl Counter {
    pub(crate) fn monotonic() -> Self {
        Self::default()
    }

    /// A counter that restarts from zero after reaching `range`.
    pub(crate) fn wrapping_at(range: u64) -> Self {
        Self {
            last: None,
            wrap_at: Some(range),
        }
    }

    /// The increase since the previous value; `None` for a baseline or when a monotonic
    /// counter went backwards (its source was reset).
    pub(crate) fn delta(&mut self, value: u64) -> Option<u64> {
        let previous = self.last.replace(value)?;
        if value >= previous {
            Some(value - previous)
        } else {
            self.wrap_at
                .map(|range| range.saturating_sub(previous) + value)
        }
    }

    pub(crate) fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_value_is_a_baseline() {
        let mut counter = Counter::monotonic();
        assert_eq!(counter.delta(100), None);
        assert_eq!(counter.delta(150), Some(50));
    }

    #[test]
    fn wrapping_counter_handles_overflow() {
        let mut counter = Counter::wrapping_at(1000);
        counter.delta(990);
        assert_eq!(counter.delta(15), Some(25));
    }

    #[test]
    fn monotonic_counter_drops_a_decrease() {
        let mut counter = Counter::monotonic();
        counter.delta(500);
        assert_eq!(counter.delta(10), None);
        assert_eq!(counter.delta(30), Some(20));
    }

    #[test]
    fn reset_starts_a_new_baseline() {
        let mut counter = Counter::monotonic();
        counter.delta(1);
        counter.reset();
        assert_eq!(counter.delta(5), None);
    }
}
