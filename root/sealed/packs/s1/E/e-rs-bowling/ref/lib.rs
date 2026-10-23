// Reference solution of e-rs-bowling (rendered by gen/run with the seeded constants).
const FRAMES: usize = @FRAMES@;
const PINS: u16 = @PINS@;
const STRIKE_BONUS: usize = @STRIKE_BONUS@;
const SPARE_BONUS: usize = @SPARE_BONUS@;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    NotEnoughPinsLeft,
    GameComplete,
}

pub struct BowlingGame {
    rolls: Vec<u16>,
    frame: usize,          // 1-based frame being played
    second_roll: bool,     // the next roll is the second of a frame
    standing: u16,         // pins standing for the next roll
    fill_left: Option<usize>,
    done: bool,
}

impl Default for BowlingGame {
    fn default() -> Self {
        Self::new()
    }
}

impl BowlingGame {
    pub fn new() -> Self {
        BowlingGame { rolls: Vec::new(), frame: 1, second_roll: false, standing: PINS, fill_left: None, done: false }
    }

    fn end_frame(&mut self, bonus_rolls: usize) {
        self.second_roll = false;
        self.standing = PINS;
        if self.frame == FRAMES {
            if bonus_rolls == 0 {
                self.done = true;
            } else {
                self.fill_left = Some(bonus_rolls);
            }
        } else {
            self.frame += 1;
        }
    }

    pub fn roll(&mut self, pins: u16) -> Result<(), Error> {
        if self.done {
            return Err(Error::GameComplete);
        }
        if pins > self.standing {
            return Err(Error::NotEnoughPinsLeft);
        }
        self.rolls.push(pins);
        if let Some(left) = self.fill_left {
            self.standing -= pins;
            if self.standing == 0 {
                self.standing = PINS;
            }
            self.fill_left = Some(left - 1);
            if left == 1 {
                self.done = true;
            }
            return Ok(());
        }
        if !self.second_roll {
            if pins == PINS {
                self.end_frame(STRIKE_BONUS);
            } else {
                self.standing = PINS - pins;
                self.second_roll = true;
            }
        } else if pins == self.standing {
            self.end_frame(SPARE_BONUS);
        } else {
            self.end_frame(0);
        }
        Ok(())
    }

    pub fn score(&self) -> Option<u16> {
        if !self.done {
            return None;
        }
        let r = &self.rolls;
        let mut total = 0u16;
        let mut i = 0;
        for _ in 0..FRAMES {
            if r[i] == PINS {
                // the next STRIKE_BONUS rolls, or all remaining rolls if fewer follow
                total += PINS + r.iter().skip(i + 1).take(STRIKE_BONUS).sum::<u16>();
                i += 1;
            } else if r[i] + r[i + 1] == PINS {
                total += PINS + r.iter().skip(i + 2).take(SPARE_BONUS).sum::<u16>();
                i += 2;
            } else {
                total += r[i] + r[i + 1];
                i += 2;
            }
        }
        Some(total)
    }
}
