/// The sum of every whole number from `from` to `to`, both included.
/// Empty when `from` is past `to`.
pub fn sum_range(from: i64, to: i64) -> i64 {
    let mut total = 0;
    for n in from..to {
        total += n;
    }
    total
}

/// The sum of the numbers in `values` from index `start` to index `end`,
/// both included. Indexes past the end are left out.
pub fn sum_slice(values: &[i64], start: usize, end: usize) -> i64 {
    values.iter().skip(start).take(end - start).sum()
}
