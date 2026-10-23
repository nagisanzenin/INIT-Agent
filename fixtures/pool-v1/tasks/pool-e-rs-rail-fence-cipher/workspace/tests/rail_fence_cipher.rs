// Visible subset (3 of 7) of the exercise's tests.
use rail_fence_cipher::RailFence;

#[test]
fn encode_with_two_rails() {
    let input = "XOXOXOXOXOXOXOXOXO";
    let rails = 2;
    let rail_fence = RailFence::new(rails);
    let output = rail_fence.encode(input);
    let expected = "XXXXXXXXXOOOOOOOOO";
    assert_eq!(output, expected);
}

#[test]
fn encode_with_three_rails() {
    let input = "WEAREDISCOVEREDFLEEATONCE";
    let rails = 3;
    let rail_fence = RailFence::new(rails);
    let output = rail_fence.encode(input);
    let expected = "WECRLTEERDSOEEFEAOCAIVDEN";
    assert_eq!(output, expected);
}

#[test]
fn encode_with_ending_in_the_middle() {
    let input = "EXERCISES";
    let rails = 4;
    let rail_fence = RailFence::new(rails);
    let output = rail_fence.encode(input);
    let expected = "ESXIEECSR";
    assert_eq!(output, expected);
}

