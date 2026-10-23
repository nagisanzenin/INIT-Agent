// Visible subset (5 of 12) of the exercise's tests.
use tournament::*;

#[test]
fn just_the_header_if_no_input() {
    let input: &[&str] = &[];
    let input = input.join("\n");
    let output = tally(&input);
    let expected = ["Team                           | MP |  W |  D |  L |  P"].join("\n");
    assert_eq!(output, expected);
}

#[test]
fn a_win_is_three_points_a_loss_is_zero_points() {
    let input: &[&str] = &["Allegoric Alaskans;Blithering Badgers;win"];
    let input = input.join("\n");
    let output = tally(&input);
    let expected = [
        "Team                           | MP |  W |  D |  L |  P",
        "Allegoric Alaskans             |  1 |  1 |  0 |  0 |  3",
        "Blithering Badgers             |  1 |  0 |  0 |  1 |  0",
    ]
    .join("\n");
    assert_eq!(output, expected);
}

#[test]
fn a_win_can_also_be_expressed_as_a_loss() {
    let input: &[&str] = &["Blithering Badgers;Allegoric Alaskans;loss"];
    let input = input.join("\n");
    let output = tally(&input);
    let expected = [
        "Team                           | MP |  W |  D |  L |  P",
        "Allegoric Alaskans             |  1 |  1 |  0 |  0 |  3",
        "Blithering Badgers             |  1 |  0 |  0 |  1 |  0",
    ]
    .join("\n");
    assert_eq!(output, expected);
}

#[test]
fn a_different_team_can_win() {
    let input: &[&str] = &["Blithering Badgers;Allegoric Alaskans;win"];
    let input = input.join("\n");
    let output = tally(&input);
    let expected = [
        "Team                           | MP |  W |  D |  L |  P",
        "Blithering Badgers             |  1 |  1 |  0 |  0 |  3",
        "Allegoric Alaskans             |  1 |  0 |  0 |  1 |  0",
    ]
    .join("\n");
    assert_eq!(output, expected);
}

#[test]
fn a_draw_is_one_point_each() {
    let input: &[&str] = &["Allegoric Alaskans;Blithering Badgers;draw"];
    let input = input.join("\n");
    let output = tally(&input);
    let expected = [
        "Team                           | MP |  W |  D |  L |  P",
        "Allegoric Alaskans             |  1 |  0 |  1 |  0 |  1",
        "Blithering Badgers             |  1 |  0 |  1 |  0 |  1",
    ]
    .join("\n");
    assert_eq!(output, expected);
}

