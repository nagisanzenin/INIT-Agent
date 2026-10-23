// Reference solution of e-rs-allergies (rendered by gen/run with the seeded constants).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Allergen {
    Eggs,
    Peanuts,
    Shellfish,
    Strawberries,
    Tomatoes,
    Chocolate,
    Pollen,
    Cats,
}

/// The score is XOR-ed with this value before it is decoded (0 = unchanged).
const XOR_MASK: u32 = @XOR_MASK@;

/// (allergen, score value) in the order in which `allergies()` reports them.
const TABLE: [(Allergen, u32); 8] = [
@TABLE@
];

pub struct Allergies {
    effective: u32,
}

impl Allergies {
    pub fn new(score: u32) -> Self {
        Allergies {
            effective: score ^ XOR_MASK,
        }
    }

    pub fn is_allergic_to(&self, allergen: &Allergen) -> bool {
        TABLE
            .iter()
            .any(|(a, value)| a == allergen && self.effective & value != 0)
    }

    pub fn allergies(&self) -> Vec<Allergen> {
        TABLE
            .iter()
            .filter(|(_, value)| self.effective & value != 0)
            .map(|(a, _)| *a)
            .collect()
    }
}
