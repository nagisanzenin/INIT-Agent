// Visible subset (10 of 29) of the exercise's tests.
use yacht::*;

#[test]
fn yacht() {
    let expected = 50;
    assert_eq!(score([5, 5, 5, 5, 5], Category::Yacht), expected);
}

#[test]
fn not_yacht() {
    let expected = 0;
    assert_eq!(score([1, 3, 3, 2, 5], Category::Yacht), expected);
}

#[test]
fn ones() {
    let expected = 3;
    assert_eq!(score([1, 1, 1, 3, 5], Category::Ones), expected);
}

#[test]
fn ones_out_of_order() {
    let expected = 3;
    assert_eq!(score([3, 1, 1, 5, 1], Category::Ones), expected);
}

#[test]
fn no_ones() {
    let expected = 0;
    assert_eq!(score([4, 3, 6, 5, 5], Category::Ones), expected);
}

#[test]
fn twos() {
    let expected = 2;
    assert_eq!(score([2, 3, 4, 5, 6], Category::Twos), expected);
}

#[test]
fn fours() {
    let expected = 8;
    assert_eq!(score([1, 4, 1, 4, 1], Category::Fours), expected);
}

#[test]
fn yacht_counted_as_threes() {
    let expected = 15;
    assert_eq!(score([3, 3, 3, 3, 3], Category::Threes), expected);
}

#[test]
fn yacht_of_3s_counted_as_fives() {
    let expected = 0;
    assert_eq!(score([3, 3, 3, 3, 3], Category::Fives), expected);
}

#[test]
fn fives() {
    let expected = 10;
    assert_eq!(score([1, 5, 3, 5, 3], Category::Fives), expected);
}

