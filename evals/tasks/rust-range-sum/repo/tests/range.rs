use range_sum::{sum_range, sum_slice};

#[test]
fn ranges_include_both_ends() {
    assert_eq!(sum_range(1, 3), 6);
    assert_eq!(sum_range(5, 5), 5);
    assert_eq!(sum_range(-2, 2), 0);
}

#[test]
fn an_empty_range_is_zero() {
    assert_eq!(sum_range(3, 1), 0);
}

#[test]
fn slices_include_both_ends() {
    assert_eq!(sum_slice(&[1, 2, 3, 4], 1, 2), 5);
    assert_eq!(sum_slice(&[1, 2, 3, 4], 0, 0), 1);
    assert_eq!(sum_slice(&[1, 2, 3, 4], 2, 10), 7);
}
