// dev fixture (partial): the number categories, Choice and Yacht only; every other category scores 0.
#[derive(Debug)]
pub enum Category {
    Ones,
    Twos,
    Threes,
    Fours,
    Fives,
    Sixes,
    FullHouse,
    FourOfAKind,
    LittleStraight,
    BigStraight,
    Choice,
    Yacht,
}

type Dice = [u8; 5];

pub fn score(dice: Dice, category: Category) -> u8 {
    let count = |n: u8| dice.iter().filter(|&&d| d == n).count() as u8;
    match category {
        Category::Ones => count(1),
        Category::Twos => 2 * count(2),
        Category::Threes => 3 * count(3),
        Category::Fours => 4 * count(4),
        Category::Fives => 5 * count(5),
        Category::Sixes => 6 * count(6),
        Category::Choice => dice.iter().sum(),
        Category::Yacht => if dice.iter().all(|&d| d == dice[0]) { 50 } else { 0 },
        _ => 0,
    }
}
